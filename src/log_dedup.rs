//! Junta líneas de log idénticas y seguidas: `mensaje(100)` en vez de cien líneas.

use log::{Level, Log, Metadata, Record};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const REPEAT_IDLE: Duration = Duration::from_millis(100);

struct Pending {
    level: Level,
    target: String,
    module: Option<String>,
    file: Option<String>,
    line: Option<u32>,
    /// Texto sin la duración final, para considerar repetida la misma carga.
    key: String,
    msg: String,
    count: u64,
    last: Instant,
}

static INNER: OnceLock<env_logger::Logger> = OnceLock::new();
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

struct Dedup;

impl Log for Dedup {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        INNER.get().is_some_and(|inner| inner.enabled(metadata))
    }

    fn log(&self, record: &Record<'_>) {
        let Some(inner) = INNER.get() else {
            return;
        };
        if !inner.matches(record) {
            return;
        }
        let msg = record.args().to_string();
        let key = dedup_key(&msg).to_string();
        let mut pending = PENDING.lock().unwrap();
        if let Some(slot) = pending.as_mut()
            && slot.level == record.level()
            && slot.target == record.target()
            && slot.key == key
        {
            slot.count = slot.count.saturating_add(1);
            slot.msg = msg;
            slot.last = Instant::now();
            return;
        }
        let previous = pending.take();
        *pending = Some(Pending {
            level: record.level(),
            target: record.target().to_string(),
            module: record.module_path().map(str::to_string),
            file: record.file().map(str::to_string),
            line: record.line(),
            key,
            msg,
            count: 1,
            last: Instant::now(),
        });
        drop(pending);
        if let Some(previous) = previous {
            emit(inner, previous);
        }
    }

    fn flush(&self) {
        flush();
    }
}

pub fn init() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::new().filter_or("LIVENYAN_LOG", "warn"));
    builder.format_timestamp_millis();
    let logger = builder.build();
    log::set_max_level(logger.filter());
    let _ = INNER.set(logger);
    let _ = log::set_logger(&Dedup);
}

/// Suelta la línea retenida si lleva un rato sin repetirse.
pub fn poll() {
    let Some(inner) = INNER.get() else {
        return;
    };
    let mut pending = PENDING.lock().unwrap();
    let idle = pending
        .as_ref()
        .is_some_and(|slot| slot.last.elapsed() >= REPEAT_IDLE);
    if !idle {
        return;
    }
    let Some(slot) = pending.take() else {
        return;
    };
    drop(pending);
    emit(inner, slot);
}

/// Suelta la línea retenida ahora (al salir).
pub fn flush() {
    let Some(inner) = INNER.get() else {
        return;
    };
    let slot = PENDING.lock().unwrap().take();
    if let Some(slot) = slot {
        emit(inner, slot);
    }
    inner.flush();
}

fn emit(inner: &env_logger::Logger, slot: Pending) {
    let text = if slot.count > 1 {
        format!("{}({})", slot.key, slot.count)
    } else {
        slot.msg
    };
    inner.log(
        &Record::builder()
            .args(format_args!("{text}"))
            .level(slot.level)
            .target(&slot.target)
            .module_path(slot.module.as_deref())
            .file(slot.file.as_deref())
            .line(slot.line)
            .build(),
    );
}

/// `… in 1.2 ms` no distingue dos cargas del mismo recurso.
fn dedup_key(msg: &str) -> &str {
    let Some(at) = msg.rfind(" in ") else {
        return msg;
    };
    let tail = &msg[at + 4..];
    let Some(number) = tail.strip_suffix(" ms") else {
        return msg;
    };
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return msg;
    }
    &msg[..at]
}

#[cfg(test)]
mod tests {
    use super::dedup_key;

    #[test]
    fn timing_suffix_does_not_split_the_same_load() {
        assert_eq!(
            dedup_key("load image a.gal: 8x8, 1 frame(s) in 1.2 ms"),
            "load image a.gal: 8x8, 1 frame(s)"
        );
        assert_eq!(
            dedup_key("enter script a.lsb:0 (jump)"),
            "enter script a.lsb:0 (jump)"
        );
    }
}
