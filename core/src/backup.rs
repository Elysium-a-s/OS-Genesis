//! Konzistentná záloha databázy za behu.
//!
//! Celý stav Genesis je jeden SQLite súbor: povely a ich audit, časovo obmedzené
//! granty, hlasové záznamy a vydané kreditívy. Skopírovať ho za behu `cp`-om je
//! chyba — kópia môže vzniknúť v strede zápisu a byť poškodená. `VACUUM INTO`
//! naopak vypíše obsah pod čítacou transakciou, takže výsledok je celý a platný
//! aj vtedy, keď sa práve zapisuje.
//!
//! Obnova sem nepatrí a patriť nemôže: služba nedokáže bezpečne podsunúť súbor
//! sama sebe pod otvoreným spojením. Postup obnovy je v `core/README.md` a robí
//! ho prevádzkovateľ pri zastavenej službe.

use std::{fs, path::Path};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use crate::ledger::{Ledger, LedgerError};

/// Kde a aká záloha vznikla.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Backup {
    pub path: String,
    pub bytes: u64,
    pub at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("ledger error: {0}")]
    Ledger(#[from] LedgerError),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("backup directory is unusable: {0}")]
    Directory(#[from] std::io::Error),
    #[error("a backup for this moment already exists")]
    AlreadyExists,
}

/// Vypíše konzistentnú kópiu databázy do priečinka.
///
/// Meno nesie čas vzniku a dvojbodky v ňom sú nahradené, aby sa dalo uložiť aj
/// tam, kde sú zakázané. Existujúci súbor sa neprepíše — záloha, ktorá by
/// prepísala predchádzajúcu, je horšia než chýbajúca.
pub fn write(ledger: &Ledger, directory: &Path, now: DateTime<Utc>) -> Result<Backup, BackupError> {
    fs::create_dir_all(directory)?;
    let at = now.to_rfc3339_opts(SecondsFormat::Millis, true);
    let path = directory.join(file_name(&at));
    if path.exists() {
        return Err(BackupError::AlreadyExists);
    }
    // Cesta ide ako parameter, nie do textu dotazu; `VACUUM INTO` ju prijme ako
    // výraz, takže sa nemusí nič escapovať.
    ledger
        .connection()
        .execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
    let bytes = fs::metadata(&path)?.len();
    Ok(Backup {
        path: path.to_string_lossy().into_owned(),
        bytes,
        at,
    })
}

/// Koľko záloh prehľad vráti. Odpoveď nesmie rásť bez hranice.
pub const LIST_LIMIT: usize = 50;

const PREFIX: &str = "genesis-ledger-";
const SUFFIX: &str = ".sqlite3";

/// Meno súboru zálohy pre daný okamih.
///
/// Dvojbodky sa nahrádzajú, aby sa dalo uložiť aj na súborové systémy, ktoré ich
/// nepovoľujú. Je to jediné miesto, kde sa meno skladá — `list` ho tým istým
/// pravidlom rozoberá, takže sa tie dve nemôžu rozísť.
fn file_name(at: &str) -> String {
    format!("{PREFIX}{}{SUFFIX}", at.replace(':', "-"))
}

/// Okamih z mena súboru, alebo `None`, keď to záloha Genesis nie je.
///
/// Dvojbodky sa vracajú iba do časovej časti za `T`. Slepé `-` → `:` by zlomilo
/// dátum, ktorý spojovníky nesie legitímne. Čo sa nedá prečítať ako RFC 3339, sa
/// zahodí: cudzí súbor v priečinku nie je záloha a hlásiť ho ako zálohu by bolo
/// horšie než ho nevidieť.
fn moment_from(name: &str) -> Option<String> {
    let stem = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let (date, time) = stem.split_once('T')?;
    let at = format!("{date}T{}", time.replace('-', ":"));
    DateTime::parse_from_rfc3339(&at).ok()?;
    Some(at)
}

/// Zálohy v priečinku, najnovšia prvá.
///
/// Čas sa berie z mena súboru, nie z času úpravy: ten sa dá zmeniť kopírovaním aj
/// `touch`-om, kým meno hovorí, kedy záloha skutočne vznikla.
///
/// Chýbajúci priečinok je prázdny zoznam, nie chyba — znamená to, že záloha ešte
/// nebola, čo je legitímny stav jednotky.
pub fn list(directory: &Path) -> Result<Vec<Backup>, BackupError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(at) = name.to_str().and_then(moment_from) else {
            continue;
        };
        found.push(Backup {
            path: entry.path().to_string_lossy().into_owned(),
            bytes: entry.metadata()?.len(),
            at,
        });
    }
    // Mená majú rovnaký tvar a pevnú dĺžku, takže reťazcové zoradenie je
    // chronologické.
    found.sort_by(|left, right| right.at.cmp(&left.at));
    found.truncate(LIST_LIMIT);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Actor, ActorType, CommandRequest, Status};
    use serde_json::Value;

    fn at(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn owner() -> Actor {
        Actor {
            actor_type: ActorType::User,
            actor_id: "pilot-owner".to_owned(),
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("genesis-backup-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    fn with_one_command(path: &Path) -> Ledger {
        let mut ledger = Ledger::open(path).unwrap();
        ledger
            .accept(CommandRequest {
                household_id: "pilot-home".to_owned(),
                command_id: "cmd:1".to_owned(),
                device_id: "ha:light.living".to_owned(),
                capability_id: "power".to_owned(),
                value: Value::Bool(true),
                actor: owner(),
                idempotency_key: "idem-1".to_owned(),
                correlation_id: "corr-1".to_owned(),
            })
            .unwrap();
        ledger
    }

    #[test]
    fn a_backup_can_be_opened_and_carries_the_commands() {
        let directory = scratch("restore");
        let live = directory.join("live.sqlite3");
        fs::create_dir_all(&directory).unwrap();
        let ledger = with_one_command(&live);

        let backup = write(
            &ledger,
            &directory.join("backups"),
            at("2026-09-30T08:00:00Z"),
        )
        .unwrap();
        assert!(backup.bytes > 0);
        assert!(backup
            .path
            .contains("genesis-ledger-2026-09-30T08-00-00.000Z"));

        // Záloha je plnohodnotná databáza, nie iba súbor: dá sa otvoriť a
        // obsahuje to, čo mala.
        let restored = Ledger::open(&backup.path).unwrap();
        assert_eq!(
            restored.get("cmd:1").unwrap().unwrap().status,
            Status::Accepted
        );
        drop(ledger);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_second_backup_of_the_same_moment_is_refused() {
        let directory = scratch("collision");
        let live = directory.join("live.sqlite3");
        fs::create_dir_all(&directory).unwrap();
        let ledger = with_one_command(&live);
        let moment = at("2026-09-30T08:00:00Z");
        let backups = directory.join("backups");

        write(&ledger, &backups, moment).unwrap();
        // Prepísať predchádzajúcu zálohu je horšie než ju nevytvoriť.
        assert!(matches!(
            write(&ledger, &backups, moment),
            Err(BackupError::AlreadyExists)
        ));
        // Iný okamih je iný súbor.
        assert!(write(&ledger, &backups, at("2026-09-30T08:00:01Z")).is_ok());
        drop(ledger);
        fs::remove_dir_all(&directory).unwrap();
    }
    /// Meno súboru a čas v ňom musia byť navzájom prevoditeľné, inak by prehľad
    /// hlásil iný čas, než kedy záloha vznikla.
    #[test]
    fn the_file_name_and_the_moment_round_trip() {
        for raw in [
            "2026-09-30T08:00:00.000Z",
            "2026-10-01T23:59:59.999Z",
            "2026-01-01T00:00:00.000Z",
        ] {
            let name = file_name(raw);
            assert!(name.starts_with(PREFIX) && name.ends_with(SUFFIX));
            // V mene nesmie zostať dvojbodka.
            assert!(!name.contains(':'), "{name} still has a colon");
            assert_eq!(moment_from(&name).as_deref(), Some(raw));
        }

        // Dátum nesie spojovníky legitímne; slepé nahradenie by ho zlomilo.
        assert_eq!(
            moment_from("genesis-ledger-2026-09-30T08-00-00.000Z.sqlite3").as_deref(),
            Some("2026-09-30T08:00:00.000Z")
        );

        // Čo nie je záloha Genesis, sa nerozoberá.
        for foreign in [
            "notes.txt",
            "genesis-ledger-.sqlite3",
            "genesis-ledger-not-a-time.sqlite3",
            "genesis-ledger-2026-09-30T08-00-00.000Z.txt",
            "other-ledger-2026-09-30T08-00-00.000Z.sqlite3",
        ] {
            assert_eq!(moment_from(foreign), None, "{foreign} was read as a backup");
        }
    }

    /// Prehľad vracia najnovšiu prvú a cudzí súbor v priečinku ignoruje.
    #[test]
    fn the_listing_is_newest_first_and_ignores_what_is_not_a_backup() {
        let directory = scratch("listing");
        let live = directory.join("live.sqlite3");
        fs::create_dir_all(&directory).unwrap();
        let ledger = with_one_command(&live);
        let backups = directory.join("backups");

        // Chýbajúci priečinok znamená, že záloha ešte nebola — nie chybu.
        assert!(list(&backups).unwrap().is_empty());

        let first = write(&ledger, &backups, at("2026-09-30T08:00:00Z")).unwrap();
        let second = write(&ledger, &backups, at("2026-09-30T09:00:00Z")).unwrap();
        fs::write(backups.join("notes.txt"), b"nie zaloha").unwrap();
        fs::create_dir_all(backups.join("genesis-ledger-dir.sqlite3")).unwrap();

        let found = list(&backups).unwrap();
        assert_eq!(found.len(), 2, "foreign entries leaked into the listing");
        assert_eq!(found[0].at, second.at);
        assert_eq!(found[1].at, first.at);
        assert_eq!(found[0].path, second.path);
        assert!(found[0].bytes > 0);
        // Čas v prehľade je ten, ktorý zápis vrátil.
        assert_eq!(found[1].bytes, first.bytes);

        drop(ledger);
        fs::remove_dir_all(&directory).unwrap();
    }
}
