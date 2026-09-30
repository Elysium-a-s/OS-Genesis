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
    let path = directory.join(format!("genesis-ledger-{}.sqlite3", at.replace(':', "-")));
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
}
