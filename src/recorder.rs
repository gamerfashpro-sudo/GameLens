use crate::{collector::Collector, paths, store};
use std::time::Duration;

/// Фоновый рекордер для игрового режима: без окна, без истории в памяти.
/// Замер раз в 2 с; останавливается, когда в интерфейсе нажали «Остановить» (появляется stop-файл).
pub fn run() {
    if paths::recorder_running() {
        return; // уже идёт запись
    }
    let _ = std::fs::create_dir_all(paths::data_dir());
    let _ = std::fs::remove_file(paths::stop_file());
    let _ = std::fs::write(paths::pid_file(), std::process::id().to_string());
    let _ = store::prune(&paths::sessions_dir(), 30);

    let name = format!("session-{}.jsonl", chrono_like_stamp());
    let path = paths::sessions_dir().join(name);
    let Ok(mut w) = store::Writer::create(&path) else { return };
    let mut c = Collector::new(5);
    loop {
        std::thread::sleep(Duration::from_millis(2000));
        if paths::stop_file().exists() {
            break;
        }
        if w.write(&c.sample()).is_err() {
            break;
        }
    }
    let _ = std::fs::remove_file(paths::stop_file());
    let _ = std::fs::remove_file(paths::pid_file());
}

fn chrono_like_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // сортируемое имя: число секунд с 1970 (дополнено нулями)
    format!("{:012}", secs)
}
