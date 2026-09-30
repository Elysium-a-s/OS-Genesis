//! Párovanie, roly a správa tajomstiev.
//!
//! Prístup ku Genesis sa nezískava tým, že si niekto prečíta konfiguráciu.
//! Vlastník domácnosti spustí párovanie, dostane **jednorazový kód**, a ten sa
//! raz vymení za prístupový token. Token sa dá kedykoľvek odobrať.
//!
//! Tajomstvo Genesis nikdy neukladá v čitateľnej podobe: v databáze je iba
//! SHA-256 odtlačok kódu a tokenu. Kód ani token nikdy nevstúpi do logu a z API
//! sa vráti práve raz — vtedy, keď vznikne. Kto ho stratí, spraví nové
//! párovanie; kto ho vyzradí, dá ho odobrať.
//!
//! Prečo stačí jeden SHA-256 a nie KDF: tokeny nie sú heslá, sú to náhodné
//! hodnoty s viac než dvomi stovkami bitov entropie. Proti nim nemá slovníkový
//! ani hrubý útok o čo sa oprieť, takže zdržiavacia funkcia by nechránila pred
//! ničím, čo by inak hrozilo.

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::ledger::{Ledger, LedgerError};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS pairings (
       pairing_id TEXT PRIMARY KEY,
       household_id TEXT NOT NULL,
       role TEXT NOT NULL,
       actor_id TEXT NOT NULL,
       code_hash TEXT NOT NULL UNIQUE,
       created_by TEXT NOT NULL,
       created_at TEXT NOT NULL,
       expires_at TEXT NOT NULL,
       redeemed_at TEXT
     );
     CREATE TABLE IF NOT EXISTS credentials (
       credential_id TEXT PRIMARY KEY,
       household_id TEXT NOT NULL,
       role TEXT NOT NULL,
       actor_id TEXT NOT NULL,
       token_hash TEXT NOT NULL UNIQUE,
       pairing_id TEXT NOT NULL REFERENCES pairings(pairing_id),
       issued_at TEXT NOT NULL,
       last_used_at TEXT,
       revoked_at TEXT,
       revoked_by TEXT
     );
     CREATE INDEX IF NOT EXISTS credentials_by_token ON credentials(token_hash);
     CREATE INDEX IF NOT EXISTS credentials_by_household
       ON credentials(household_id, issued_at);";

/// Ako dlho sa dá jednorazový kód vymeniť. Párovanie je úkon, ktorý niekto robí
/// práve teraz, nie oprávnenie na neskôr.
const PAIRING_TTL_SECONDS: i64 = 600;

/// Koľko kreditív vráti prehľad. Odpoveď nesmie rásť bez hranice.
const CREDENTIAL_LIMIT: i64 = 200;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Member,
    Guest,
    Service,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
            Self::Guest => "guest",
            Self::Service => "service",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "owner" => Some(Self::Owner),
            "member" => Some(Self::Member),
            "guest" => Some(Self::Guest),
            "service" => Some(Self::Service),
            _ => None,
        }
    }

    /// Ovládať zariadenia smie owner a member. Guest a service čítajú.
    pub fn can_control_devices(self) -> bool {
        matches!(self, Self::Owner | Self::Member)
    }
}

/// Kto požiadavku poslal, podľa vydanej kreditívy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Identity {
    pub credential_id: String,
    pub household_id: String,
    pub role: Role,
    pub actor_id: String,
}

/// Rozbehnuté párovanie. `code` je v odpovedi práve raz a nikde inde.
#[derive(Debug, Serialize)]
pub struct StartedPairing {
    pub pairing_id: String,
    pub role: Role,
    pub actor_id: String,
    pub expires_at: String,
    pub code: String,
}

/// Vydaný token. Rovnako ako kód sa vracia práve raz.
#[derive(Debug, Serialize)]
pub struct IssuedCredential {
    pub credential_id: String,
    pub household_id: String,
    pub role: Role,
    pub actor_id: String,
    pub token: String,
}

/// Čo o kreditíve smie vlastník vidieť. Token medzi tým nie je.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CredentialView {
    pub credential_id: String,
    pub household_id: String,
    pub role: Role,
    pub actor_id: String,
    pub issued_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub revoked_by: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("ledger error: {0}")]
    Ledger(#[from] LedgerError),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("stored credential is unreadable: {0}")]
    Corrupt(&'static str),
}

/// Spustí párovanie a vráti jednorazový kód.
pub fn start_pairing(
    ledger: &mut Ledger,
    household_id: &str,
    role: Role,
    actor_id: &str,
    created_by: &str,
    now: DateTime<Utc>,
) -> Result<StartedPairing, IdentityError> {
    let pairing = StartedPairing {
        pairing_id: Uuid::new_v4().to_string(),
        role,
        actor_id: actor_id.to_owned(),
        expires_at: stamp(now + TimeDelta::seconds(PAIRING_TTL_SECONDS)),
        code: secret(),
    };
    ledger.connection().execute(
        "INSERT INTO pairings
         (pairing_id, household_id, role, actor_id, code_hash, created_by, created_at,
          expires_at, redeemed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        params![
            pairing.pairing_id,
            household_id,
            role.as_str(),
            pairing.actor_id,
            fingerprint(pairing.code.as_bytes()),
            created_by,
            stamp(now),
            pairing.expires_at
        ],
    )?;
    Ok(pairing)
}

/// Vymení jednorazový kód za prístupový token.
///
/// `Ok(None)` znamená, že taký kód nie je použiteľný — neexistuje, patrí inej
/// domácnosti, uplynul mu čas alebo bol už použitý. Volajúci sa nedozvie, ktoré
/// z toho platí; kód je oprávnenie, nie meno záznamu.
pub fn redeem(
    ledger: &mut Ledger,
    household_id: &str,
    code: &str,
    now: DateTime<Utc>,
) -> Result<Option<IssuedCredential>, IdentityError> {
    let hash = fingerprint(code.as_bytes());
    let found: Option<(String, String, String, String, Option<String>)> = ledger
        .connection()
        .query_row(
            "SELECT pairing_id, household_id, role, actor_id, redeemed_at
             FROM pairings WHERE code_hash = ?1 AND expires_at > ?2",
            params![hash, stamp(now)],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((pairing_id, pairing_household, raw_role, actor_id, redeemed_at)) = found else {
        return Ok(None);
    };
    if redeemed_at.is_some() || pairing_household != household_id {
        return Ok(None);
    }
    let role = Role::parse(&raw_role).ok_or(IdentityError::Corrupt("unknown pairing role"))?;

    // Kód sa spotrebuje podmienene: dva súbežné pokusy vymeniť ten istý kód
    // skončia tak, že token dostane práve jeden.
    let claimed = ledger.connection().execute(
        "UPDATE pairings SET redeemed_at = ?2 WHERE pairing_id = ?1 AND redeemed_at IS NULL",
        params![pairing_id, stamp(now)],
    )?;
    if claimed == 0 {
        return Ok(None);
    }

    let issued = IssuedCredential {
        credential_id: Uuid::new_v4().to_string(),
        household_id: pairing_household,
        role,
        actor_id,
        token: secret(),
    };
    ledger.connection().execute(
        "INSERT INTO credentials
         (credential_id, household_id, role, actor_id, token_hash, pairing_id, issued_at,
          last_used_at, revoked_at, revoked_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, NULL)",
        params![
            issued.credential_id,
            issued.household_id,
            role.as_str(),
            issued.actor_id,
            fingerprint(issued.token.as_bytes()),
            pairing_id,
            stamp(now)
        ],
    )?;
    Ok(Some(issued))
}

/// Overí predložený token.
///
/// Hľadá sa odtlačok, nie tajomstvo: v databáze sa porovnáva SHA-256 kódu, takže
/// ani úplný výpis tabuľky nedá token, ktorým sa dá vojsť. Kreditíva pre inú
/// domácnosť, než akú spravuje táto jednotka, sa neprijme — presne to je prípad
/// prenesenej databázy, ktorému má párovanie zabrániť.
pub fn authenticate(
    ledger: &mut Ledger,
    unit_household_id: &str,
    presented: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<Identity>, IdentityError> {
    let hash = fingerprint(presented);
    let found: Option<(String, String, String, String)> = ledger
        .connection()
        .query_row(
            "SELECT credential_id, household_id, role, actor_id
             FROM credentials WHERE token_hash = ?1 AND revoked_at IS NULL",
            [hash],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((credential_id, household_id, raw_role, actor_id)) = found else {
        return Ok(None);
    };
    if household_id != unit_household_id {
        tracing::warn!(
            credential_id,
            "credential for another household presented; refused"
        );
        return Ok(None);
    }
    let role = Role::parse(&raw_role).ok_or(IdentityError::Corrupt("unknown credential role"))?;
    ledger.connection().execute(
        "UPDATE credentials SET last_used_at = ?2 WHERE credential_id = ?1",
        params![credential_id, stamp(now)],
    )?;
    Ok(Some(Identity {
        credential_id,
        household_id,
        role,
        actor_id,
    }))
}

/// Odoberie prístup. Vráti `false`, keď taká kreditíva v domácnosti nie je alebo
/// už odobraná bola.
pub fn revoke(
    ledger: &mut Ledger,
    household_id: &str,
    credential_id: &str,
    revoked_by: &str,
    now: DateTime<Utc>,
) -> Result<bool, IdentityError> {
    let changed = ledger.connection().execute(
        "UPDATE credentials SET revoked_at = ?3, revoked_by = ?4
         WHERE credential_id = ?1 AND household_id = ?2 AND revoked_at IS NULL",
        params![credential_id, household_id, stamp(now), revoked_by],
    )?;
    Ok(changed == 1)
}

/// Vydané kreditívy domácnosti, od najnovšej. Token medzi nimi nie je.
pub fn credentials(
    ledger: &Ledger,
    household_id: &str,
) -> Result<Vec<CredentialView>, IdentityError> {
    let mut statement = ledger.connection().prepare(
        "SELECT credential_id, household_id, role, actor_id, issued_at, last_used_at,
                revoked_at, revoked_by
         FROM credentials WHERE household_id = ?1
         ORDER BY issued_at DESC, credential_id LIMIT ?2",
    )?;
    let rows = statement.query_map(params![household_id, CREDENTIAL_LIMIT], |row| {
        Ok((
            CredentialView {
                credential_id: row.get(0)?,
                household_id: row.get(1)?,
                role: Role::Service,
                actor_id: row.get(3)?,
                issued_at: row.get(4)?,
                last_used_at: row.get(5)?,
                revoked_at: row.get(6)?,
                revoked_by: row.get(7)?,
            },
            row.get::<_, String>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (mut view, raw_role) = row?;
        view.role =
            Role::parse(&raw_role).ok_or(IdentityError::Corrupt("unknown credential role"))?;
        Ok(view)
    })
    .collect()
}

/// Náhodné tajomstvo v hexadecimálnom zápise. Dve UUID v4 dávajú vyše dvoch
/// stoviek bitov entropie a zápis prejde kontrolou identifikátorov.
fn secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Odtlačok tajomstva. Do databázy sa ukladá iba toto.
fn fingerprint(secret: &[u8]) -> String {
    let digest = Sha256::digest(secret);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn stamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUSE: &str = "pilot-home";

    fn at(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn ledger() -> Ledger {
        Ledger::open(":memory:").unwrap()
    }

    /// Všetko, čo o párovaniach a kreditívach v databáze skutočne je.
    fn everything_stored(ledger: &Ledger) -> String {
        let pairings: String = ledger
            .connection()
            .query_row(
                "SELECT COALESCE(group_concat(pairing_id || household_id || role || actor_id ||
                     code_hash || created_by || created_at || expires_at ||
                     COALESCE(redeemed_at, '')), '') FROM pairings",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let credentials: String = ledger
            .connection()
            .query_row(
                "SELECT COALESCE(group_concat(credential_id || household_id || role || actor_id ||
                     token_hash || pairing_id || issued_at || COALESCE(last_used_at, '') ||
                     COALESCE(revoked_at, '') || COALESCE(revoked_by, '')), '') FROM credentials",
                [],
                |row| row.get(0),
            )
            .unwrap();
        format!("{pairings}{credentials}")
    }

    fn pair(ledger: &mut Ledger, role: Role, now: DateTime<Utc>) -> StartedPairing {
        start_pairing(ledger, HOUSE, role, "lukas", "pilot-owner", now).unwrap()
    }

    #[test]
    fn a_code_becomes_a_credential_exactly_once() {
        let mut ledger = ledger();
        let now = at("2026-09-30T08:00:00Z");
        let pairing = pair(&mut ledger, Role::Member, now);

        let issued = redeem(&mut ledger, HOUSE, &pairing.code, now)
            .unwrap()
            .unwrap();
        assert_eq!(issued.role, Role::Member);
        assert_eq!(issued.actor_id, "lukas");
        assert_eq!(issued.household_id, HOUSE);
        assert_ne!(issued.token, pairing.code);

        // Jednorazový znamená jednorazový.
        assert!(redeem(&mut ledger, HOUSE, &pairing.code, now)
            .unwrap()
            .is_none());
        // Cudzí kód nedá nič.
        assert!(redeem(&mut ledger, HOUSE, "nieco-ine", now)
            .unwrap()
            .is_none());
    }

    #[test]
    fn neither_the_code_nor_the_token_is_stored_in_a_readable_form() {
        let mut ledger = ledger();
        let now = at("2026-09-30T08:00:00Z");
        let pairing = pair(&mut ledger, Role::Owner, now);
        let issued = redeem(&mut ledger, HOUSE, &pairing.code, now)
            .unwrap()
            .unwrap();

        let stored = everything_stored(&ledger);
        assert!(
            !stored.contains(&pairing.code),
            "the pairing code is stored"
        );
        assert!(
            !stored.contains(&issued.token),
            "the access token is stored"
        );
        // Odtlačok tam naopak byť musí, inak by sa token nedal overiť.
        assert!(stored.contains(&fingerprint(issued.token.as_bytes())));
    }

    #[test]
    fn an_expired_or_foreign_code_is_not_redeemable() {
        let mut ledger = ledger();
        let now = at("2026-09-30T08:00:00Z");
        let pairing = pair(&mut ledger, Role::Member, now);
        // Po uplynutí času sa kód vymeniť nedá.
        assert!(redeem(
            &mut ledger,
            HOUSE,
            &pairing.code,
            now + TimeDelta::seconds(PAIRING_TTL_SECONDS + 1)
        )
        .unwrap()
        .is_none());

        let other = pair(&mut ledger, Role::Member, now);
        // A kód tejto domácnosti nevydá kreditívu pre inú.
        assert!(redeem(&mut ledger, "other-home", &other.code, now)
            .unwrap()
            .is_none());
        // Pokus o cudziu domácnosť kód nespotreboval.
        assert!(redeem(&mut ledger, HOUSE, &other.code, now)
            .unwrap()
            .is_some());
    }

    #[test]
    fn a_credential_from_another_household_never_authenticates() {
        let mut ledger = ledger();
        let now = at("2026-09-30T08:00:00Z");
        let pairing = pair(&mut ledger, Role::Owner, now);
        let issued = redeem(&mut ledger, HOUSE, &pairing.code, now)
            .unwrap()
            .unwrap();

        // Ten istý token na jednotke, ktorá spravuje inú domácnosť — presne
        // prípad prenesenej databázy — neprejde.
        assert!(
            authenticate(&mut ledger, "other-home", issued.token.as_bytes(), now)
                .unwrap()
                .is_none()
        );
        // Na svojej jednotke prejde.
        let identity = authenticate(&mut ledger, HOUSE, issued.token.as_bytes(), now)
            .unwrap()
            .unwrap();
        assert_eq!(identity.household_id, HOUSE);
        assert_eq!(identity.role, Role::Owner);
        assert_eq!(identity.actor_id, "lukas");
        assert_eq!(identity.credential_id, issued.credential_id);
    }

    #[test]
    fn revocation_stops_the_token_and_is_visible_to_the_owner() {
        let mut ledger = ledger();
        let now = at("2026-09-30T08:00:00Z");
        let pairing = pair(&mut ledger, Role::Guest, now);
        let issued = redeem(&mut ledger, HOUSE, &pairing.code, now)
            .unwrap()
            .unwrap();
        let used = at("2026-09-30T08:05:00Z");
        assert!(
            authenticate(&mut ledger, HOUSE, issued.token.as_bytes(), used)
                .unwrap()
                .is_some()
        );

        let revoked_at = at("2026-09-30T09:00:00Z");
        assert!(revoke(
            &mut ledger,
            HOUSE,
            &issued.credential_id,
            "pilot-owner",
            revoked_at
        )
        .unwrap());
        // Odobraný token neprejde a druhé odobranie nemá čo odobrať.
        assert!(
            authenticate(&mut ledger, HOUSE, issued.token.as_bytes(), revoked_at)
                .unwrap()
                .is_none()
        );
        assert!(!revoke(
            &mut ledger,
            HOUSE,
            &issued.credential_id,
            "pilot-owner",
            revoked_at
        )
        .unwrap());
        // Ani kreditíva inej domácnosti sa odobrať nedá.
        assert!(!revoke(
            &mut ledger,
            "other-home",
            &issued.credential_id,
            "pilot-owner",
            revoked_at
        )
        .unwrap());

        let listed = credentials(&ledger, HOUSE).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].credential_id, issued.credential_id);
        assert_eq!(listed[0].role, Role::Guest);
        assert_eq!(
            listed[0].last_used_at.as_deref(),
            Some("2026-09-30T08:05:00.000Z")
        );
        assert!(listed[0].revoked_at.is_some());
        assert_eq!(listed[0].revoked_by.as_deref(), Some("pilot-owner"));
        // Prehľad nesmie obsahovať token — Genesis ho ani nemá.
        let serialized = serde_json::to_string(&listed[0]).unwrap();
        assert!(!serialized.contains(&issued.token));
        assert!(credentials(&ledger, "other-home").unwrap().is_empty());
    }

    #[test]
    fn a_secret_is_long_random_hexadecimal() {
        let first = secret();
        let second = secret();
        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        // Odtlačok je iná hodnota než tajomstvo a je stabilný.
        assert_ne!(fingerprint(first.as_bytes()), first);
        assert_eq!(fingerprint(first.as_bytes()), fingerprint(first.as_bytes()));
    }
}
