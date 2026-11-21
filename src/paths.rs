use std::path::PathBuf;

/// Папка данных: %LOCALAPPDATA%\GameLens (на Linux для тестов — GAMELENS_DIR или ~/.local/share/GameLens)
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("GAMELENS_DIR") {
        return PathBuf::from(d);
    }
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("GameLens")
}
pub fn sessions_dir() -> PathBuf {
    data_dir().join("sessions")
}
pub fn stop_file() -> PathBuf {
    data_dir().join("stop")
}
pub fn pid_file() -> PathBuf {
    data_dir().join("recorder.pid")
}

/// Рекордер жив, если pid-файл есть и процесс с таким pid существует.
pub fn recorder_running() -> bool {
    let Ok(s) = std::fs::read_to_string(pid_file()) else { return false };
    let Ok(pid) = s.trim().parse::<u32>() else { return false };
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    sys.process(sysinfo::Pid::from_u32(pid)).is_some()
}
