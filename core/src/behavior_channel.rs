//! Autentifikovaný kanál z Elysium Behavior do Genesisu.
//!
//! Rozhodnutie Behavior enginu odomkne zariadenie v domácnosti, takže kanál musí
//! uniesť viac než „kto volá". Nesie tri veci:
//!
//! - **identitu** — ktorý kľúč požiadavku podpísal,
//! - **integritu** — že telo nikto po podpise nezmenil,
//! - **odolnosť voči zopakovaniu** — podpis staršieho tela sa nedá použiť znova.
//!
//! Preto nie holý bearer token. Token je tajomstvo, ktoré v logu proxy alebo v
//! histórii shellu stačí raz zahliadnuť a ovláda domácnosť; podpis sám o sebe
//! nie je oprávnenie na nič iné než na to jedno telo v tom jednom okne.
//!
//! Tajomstvo sa nikdy nedostane do URL (je v hlavičke, nie v query), do Gitu
//! (číta sa z prostredia) ani do logu (`Debug` ho zakrýva a overenie nevypisuje
//! očakávaný podpis).

use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

/// Cesta, na ktorú je podpis viazaný.
///
/// Je súčasťou podpisovaného textu, takže platný podpis sa nedá preniesť na iný
/// endpoint, ktorý na jednotke pribudne neskôr.
pub const DECISIONS_PATH: &str = "/v1/behavior/decisions";

/// Koľko smie byť časová značka stará alebo v budúcnosti.
///
/// Päť minút znesie rozdiel hodín medzi jednotkou a serverom Behavioru bez toho,
/// aby sa z odchyteného podpisu dal urobiť použiteľný povel o hodinu neskôr.
/// Samotná idempotencia rozhodnutia je druhá zábrana, nie táto: okno rozhoduje,
/// či sa požiadavka vôbec prijme, idempotencia rozhoduje, že sa druhý povel
/// nezaloží.
const MAXIMUM_CLOCK_SKEW_MINUTES: i64 = 5;

/// Najmenšia dĺžka spoločného tajomstva.
///
/// Tá istá hranica ako pri prístupových tokenoch jednotky: kratšie tajomstvo sa
/// dá uhádnuť a kanál by potom vyzeral zabezpečený bez toho, aby bol.
const MINIMUM_SECRET_LENGTH: usize = 32;

#[derive(Clone)]
pub struct BehaviorChannel {
    key_id: String,
    secret: Vec<u8>,
}

/// Zakrýva tajomstvo. Štruktúra sa môže dostať do chybovej správy alebo do
/// diagnostiky a derivovaný `Debug` by tam vypísal celý kľúč.
impl std::fmt::Debug for BehaviorChannel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BehaviorChannel")
            .field("key_id", &self.key_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelError {
    /// Hlavička chýba alebo nemá tvar, ktorý sa dá overiť.
    Malformed,
    /// Podpísal iný kľúč, než jednotka pozná.
    UnknownKey,
    /// Časová značka je mimo okna.
    StaleTimestamp,
    /// Podpis nesúhlasí s telom.
    BadSignature,
}

impl ChannelError {
    /// Kód pre odpoveď Behavioru. Zámerne nerozlišuje neznámy kľúč od zlého
    /// podpisu: volajúci, ktorý kľúč nemá, sa z odpovede nemá dozvedieť, ktorý
    /// z tých dvoch je bližšie k pravde.
    pub fn reason_code(self) -> &'static str {
        match self {
            ChannelError::Malformed => "signature_malformed",
            ChannelError::StaleTimestamp => "signature_timestamp_outside_window",
            ChannelError::UnknownKey | ChannelError::BadSignature => "signature_rejected",
        }
    }
}

/// Podpísaná požiadavka, tak ako prišla v hlavičkách.
pub struct SignedRequest<'a> {
    pub key_id: &'a str,
    pub timestamp: &'a str,
    pub signature: &'a str,
    pub path: &'a str,
    pub body: &'a [u8],
}

impl BehaviorChannel {
    /// Kanál z prostredia. `None` znamená, že jednotka Behavior kanál nemá — to
    /// nie je chyba konfigurácie, len jednotka bez prepojenia na Behavior.
    pub fn from_env() -> Result<Option<Self>, &'static str> {
        let key_id = std::env::var("GENESIS_BEHAVIOR_KEY_ID").ok();
        let secret = std::env::var("GENESIS_BEHAVIOR_SECRET").ok();
        match (key_id, secret) {
            (None, None) => Ok(None),
            (Some(key_id), Some(secret)) => Self::new(key_id, secret).map(Some),
            _ => Err("set both GENESIS_BEHAVIOR_KEY_ID and GENESIS_BEHAVIOR_SECRET"),
        }
    }

    pub fn new(key_id: String, secret: String) -> Result<Self, &'static str> {
        if key_id.is_empty()
            || key_id.len() > 64
            || !key_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        {
            return Err("GENESIS_BEHAVIOR_KEY_ID must be a short opaque identifier");
        }
        if secret.len() < MINIMUM_SECRET_LENGTH {
            return Err("GENESIS_BEHAVIOR_SECRET must have at least 32 characters");
        }
        Ok(Self {
            key_id,
            secret: secret.into_bytes(),
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Overí požiadavku. Nevracia očakávaný podpis a ani ho nikde nevypisuje —
    /// bol by to návod, ako podpísať to isté telo znova.
    pub fn verify(
        &self,
        request: &SignedRequest<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), ChannelError> {
        if request.key_id.is_empty() || request.timestamp.is_empty() || request.signature.is_empty()
        {
            return Err(ChannelError::Malformed);
        }
        // Kľúč sa porovnáva v konštantnom čase rovnako ako podpis: jeho názov nie
        // je tajomstvo, ale rozdiel v čase odpovede by prezradil, ktorý znak sedí.
        if !bool::from(self.key_id.as_bytes().ct_eq(request.key_id.as_bytes())) {
            return Err(ChannelError::UnknownKey);
        }
        let stamped = parse_utc(request.timestamp).ok_or(ChannelError::Malformed)?;
        let skew = Duration::minutes(MAXIMUM_CLOCK_SKEW_MINUTES);
        if stamped < now - skew || stamped > now + skew {
            return Err(ChannelError::StaleTimestamp);
        }
        let presented = decode_hex(request.signature).ok_or(ChannelError::Malformed)?;
        let expected = self.sign(request.path, request.timestamp, request.body);
        if !bool::from(presented.ct_eq(&expected)) {
            return Err(ChannelError::BadSignature);
        }
        Ok(())
    }

    /// HMAC-SHA256 nad kanonickým textom. Rovnaký výpočet robí odosielateľ.
    pub fn sign(&self, path: &str, timestamp: &str, body: &[u8]) -> Vec<u8> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC accepts a key of any length");
        mac.update(&signing_input(path, timestamp, body));
        mac.finalize().into_bytes().to_vec()
    }

    pub fn sign_hex(&self, path: &str, timestamp: &str, body: &[u8]) -> String {
        encode_hex(&self.sign(path, timestamp, body))
    }
}

/// Kanonický podpisovaný text.
///
/// Verzia je v ňom preto, aby sa dal zmeniť tvar bez toho, aby starý podpis
/// zostal platný. Metóda a cesta preto, aby sa podpis nedal preniesť na iný
/// endpoint. Časová značka preto, aby sa nedala vymeniť za novšiu.
pub fn signing_input(path: &str, timestamp: &str, body: &[u8]) -> Vec<u8> {
    let mut input = format!("v1:POST:{path}:{timestamp}:").into_bytes();
    input.extend_from_slice(body);
    input
}

fn parse_utc(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(raw: &str) -> Option<Vec<u8>> {
    if raw.len() % 2 != 0 {
        return None;
    }
    (0..raw.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&raw[index..index + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> BehaviorChannel {
        BehaviorChannel::new("behavior-1".to_owned(), "s".repeat(32)).unwrap()
    }

    fn signed<'a>(
        channel: &BehaviorChannel,
        timestamp: &'a str,
        body: &'a [u8],
        signature: &'a mut String,
    ) -> SignedRequest<'a> {
        *signature = channel.sign_hex(DECISIONS_PATH, timestamp, body);
        SignedRequest {
            key_id: "behavior-1",
            timestamp,
            signature,
            path: DECISIONS_PATH,
            body,
        }
    }

    #[test]
    fn a_secret_that_is_too_short_or_a_half_configured_channel_is_refused() {
        assert!(BehaviorChannel::new("behavior-1".to_owned(), "short".to_owned()).is_err());
        assert!(BehaviorChannel::new(String::new(), "s".repeat(32)).is_err());
        assert!(BehaviorChannel::new("bad id".to_owned(), "s".repeat(32)).is_err());
        assert!(BehaviorChannel::new("behavior-1".to_owned(), "s".repeat(32)).is_ok());
    }

    #[test]
    fn the_signature_matches_the_vector_the_sender_produces() {
        // Ten istý vektor je zapísaný v `tests/test_genesis_channel.py` v repozitári
        // FastAPI-module. Keby sa jedna strana odchýlila — iný oddeľovač, iné
        // poradie, vynechaná cesta — prestane sedieť tu, a nie až na jednotke
        // v niečí domácnosti.
        let channel = BehaviorChannel::new(
            "behavior-1".to_owned(),
            "behavior-secret-of-at-least-32-chars".to_owned(),
        )
        .unwrap();
        assert_eq!(
            channel.sign_hex(
                DECISIONS_PATH,
                "2026-09-30T18:00:00Z",
                br#"{"decision_id":"3f5a8c1e"}"#
            ),
            "6bfc41558087a53a5e328e5648b9e6d767c8db92a77358e696baeaa537bd6d6d"
        );
    }

    #[test]
    fn a_signature_covers_the_body_the_timestamp_and_the_path() {
        let channel = channel();
        let now = Utc::now();
        let timestamp = now.to_rfc3339();
        let body = br#"{"decision_id":"a"}"#;
        let mut signature = String::new();
        let request = signed(&channel, &timestamp, body, &mut signature);
        assert_eq!(channel.verify(&request, now), Ok(()));

        // Zmenené telo s tým istým podpisom neprejde.
        let altered = SignedRequest {
            body: br#"{"decision_id":"b"}"#,
            ..request
        };
        assert_eq!(
            channel.verify(&altered, now),
            Err(ChannelError::BadSignature)
        );

        // Podpis sa nedá preniesť na inú cestu.
        let moved = SignedRequest {
            path: "/v1/commands",
            ..request
        };
        assert_eq!(channel.verify(&moved, now), Err(ChannelError::BadSignature));

        // Ani na iný kľúč.
        let other_key = SignedRequest {
            key_id: "behavior-2",
            ..request
        };
        assert_eq!(
            channel.verify(&other_key, now),
            Err(ChannelError::UnknownKey)
        );
    }

    #[test]
    fn an_old_signature_stops_working_once_the_window_closes() {
        let channel = channel();
        let signed_at = Utc::now();
        let timestamp = signed_at.to_rfc3339();
        let body = br#"{"decision_id":"a"}"#;
        let mut signature = String::new();
        let request = signed(&channel, &timestamp, body, &mut signature);
        // V okne platí.
        assert_eq!(
            channel.verify(&request, signed_at + Duration::minutes(4)),
            Ok(())
        );
        // Za oknom už nie, hoci podpis je stále matematicky správny.
        assert_eq!(
            channel.verify(&request, signed_at + Duration::minutes(6)),
            Err(ChannelError::StaleTimestamp)
        );
        // Ani ďaleko v budúcnosti, aby sa nedal podpísať povel na neskôr.
        assert_eq!(
            channel.verify(&request, signed_at - Duration::minutes(6)),
            Err(ChannelError::StaleTimestamp)
        );
    }

    #[test]
    fn a_missing_or_unreadable_header_is_malformed_not_a_bad_signature() {
        let channel = channel();
        let now = Utc::now();
        let timestamp = now.to_rfc3339();
        let body = br#"{}"#;
        let empty = SignedRequest {
            key_id: "behavior-1",
            timestamp: &timestamp,
            signature: "",
            path: DECISIONS_PATH,
            body,
        };
        assert_eq!(channel.verify(&empty, now), Err(ChannelError::Malformed));
        let not_hex = SignedRequest {
            signature: "nothex!!",
            ..empty
        };
        assert_eq!(channel.verify(&not_hex, now), Err(ChannelError::Malformed));
        let not_a_time = SignedRequest {
            timestamp: "yesterday",
            signature: "00",
            ..empty
        };
        assert_eq!(
            channel.verify(&not_a_time, now),
            Err(ChannelError::Malformed)
        );
    }

    #[test]
    fn the_secret_does_not_reach_a_log_line() {
        let channel = BehaviorChannel::new("behavior-1".to_owned(), "k".repeat(40)).unwrap();
        let printed = format!("{channel:?}");
        assert!(printed.contains("behavior-1"));
        assert!(printed.contains("<redacted>"));
        assert!(!printed.contains(&"k".repeat(40)));
    }

    #[test]
    fn an_unknown_key_and_a_bad_signature_answer_the_same_thing() {
        assert_eq!(
            ChannelError::UnknownKey.reason_code(),
            ChannelError::BadSignature.reason_code()
        );
        assert_eq!(
            ChannelError::StaleTimestamp.reason_code(),
            "signature_timestamp_outside_window"
        );
    }
}
