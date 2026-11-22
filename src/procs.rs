//! Список запущенных программ с подсказками, что из них можно отключить, и само закрытие.
//!
//! Главное правило безопасности: закрыть можно только то, что точно не ломает Windows и игру.
//! Системные процессы, защита от читов, драйверы и сам RTSS закрыть нельзя ни кнопкой, ни автоматически.
//! Автоматически закрывается только то, что пользователь сам отметил галочкой.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// фоновая программа, которая игре не нужна: закрыть безопасно
    Safe,
    /// закрыть можно, но что-то потеряется (вкладки, голосовой чат, подсветка): нужно подтверждение
    Caution,
    /// не знаем, что это; закрыть можно только осознанно, с подтверждением
    Unknown,
    /// системное или нужное для игры: не закрывается никогда
    Never,
}

#[derive(Debug)]
pub struct Known {
    pub key: &'static str,
    pub title: &'static str,
    pub why: &'static str,
    pub class: Class,
}

const fn k(key: &'static str, title: &'static str, class: Class, why: &'static str) -> Known {
    Known { key, title, why, class }
}

use Class::*;

/// Известные программы. Ключ — имя процесса в нижнем регистре без «.exe».
/// Совпадение по точному имени, чтобы «steam» не задел «steamwebhelper» и наоборот.
pub const KNOWN: &[Known] = &[
    // нельзя: система, защита, игровые службы
    k("system", "Ядро Windows", Never, "Системный процесс."),
    k("registry", "Реестр Windows", Never, "Системный процесс."),
    k("smss", "Windows", Never, "Системный процесс."),
    k("csrss", "Windows", Never, "Системный процесс: его закрытие вызывает синий экран."),
    k("wininit", "Windows", Never, "Системный процесс."),
    k("winlogon", "Вход в Windows", Never, "Системный процесс."),
    k("services", "Службы Windows", Never, "Системный процесс."),
    k("lsass", "Безопасность Windows", Never, "Системный процесс."),
    k("svchost", "Службы Windows", Never, "Системные службы. Закрыть нельзя."),
    k("dwm", "Рабочий стол Windows", Never, "Рисует окна и экран."),
    k("explorer", "Проводник и панель задач", Never, "Рабочий стол и панель задач."),
    k("fontdrvhost", "Шрифты Windows", Never, "Системный процесс."),
    k("sihost", "Оболочка Windows", Never, "Системный процесс."),
    k("taskhostw", "Windows", Never, "Системный процесс."),
    k("ctfmon", "Ввод текста Windows", Never, "Языковая панель и ввод."),
    k("runtimebroker", "Windows", Never, "Системный процесс."),
    k("searchhost", "Поиск Windows", Never, "Меню «Пуск» и поиск."),
    k("startmenuexperiencehost", "Меню «Пуск»", Never, "Системный процесс."),
    k("shellexperiencehost", "Оболочка Windows", Never, "Системный процесс."),
    k("textinputhost", "Ввод текста Windows", Never, "Системный процесс."),
    k("audiodg", "Звук Windows", Never, "Без него пропадёт звук."),
    k("conhost", "Консоль Windows", Never, "Системный процесс."),
    k("dllhost", "Windows", Never, "Системный процесс."),
    k("wmiprvse", "Служба WMI", Never, "Системный процесс."),
    k("spoolsv", "Печать Windows", Never, "Системный процесс."),
    k("memcompression", "Сжатие памяти", Never, "Системный процесс."),
    k("taskmgr", "Диспетчер задач", Never, "Нужен, чтобы разбираться с процессами."),
    k("msmpeng", "Защитник Windows", Never, "Антивирус. Лучше добавить игру в исключения."),
    k("mpdefendercoreservice", "Защитник Windows", Never, "Антивирус."),
    k("nissrv", "Защитник Windows", Never, "Антивирус."),
    k("securityhealthservice", "Безопасность Windows", Never, "Системная служба."),
    k("securityhealthsystray", "Безопасность Windows", Never, "Системная служба."),
    k("gamelens", "GameLens", Never, "Эта программа."),
    k("rtss", "RivaTuner (RTSS)", Never, "Рисует FPS поверх игры, GameLens берёт у него FPS."),
    k("rtsshooksloader", "RivaTuner (RTSS)", Never, "Нужен для надписи поверх игры."),
    k("rtsshooksloader64", "RivaTuner (RTSS)", Never, "Нужен для надписи поверх игры."),
    k("encodeserver", "RivaTuner (RTSS)", Never, "Часть RTSS."),
    k("encodeserver64", "RivaTuner (RTSS)", Never, "Часть RTSS."),
    k("msiafterburner", "MSI Afterburner", Never, "Разгон и мониторинг, обычно нужен во время игры."),
    k("easyanticheat", "Защита от читов", Never, "Без неё игра не запустится или выкинет из матча."),
    k("easyanticheat_eos", "Защита от читов", Never, "Без неё игра не запустится или выкинет из матча."),
    k("beservice", "Защита от читов BattlEye", Never, "Без неё игра не запустится или выкинет из матча."),
    k("vgc", "Vanguard (Valorant)", Never, "Защита от читов."),
    k("vgtray", "Vanguard (Valorant)", Never, "Защита от читов."),
    k("faceitclient", "FACEIT", Never, "Защита от читов."),
    k("nvdisplay.container", "Драйвер NVIDIA", Never, "Без него может пропасть панель управления и настройки видеокарты."),
    k("rtkauduservice64", "Звук Realtek", Never, "Драйвер звука."),
    // осторожно: закрыть можно, но будут потери
    k("chrome", "Браузер Chrome", Caution, "Закроются все вкладки и окна. Закрывай, если всё сохранено."),
    k("msedge", "Браузер Edge", Caution, "Закроются все вкладки и окна."),
    k("firefox", "Браузер Firefox", Caution, "Закроются все вкладки и окна."),
    k("opera", "Браузер Opera", Caution, "Закроются все вкладки и окна."),
    k("brave", "Браузер Brave", Caution, "Закроются все вкладки и окна."),
    k("browser", "Яндекс Браузер", Caution, "Закроются все вкладки и окна."),
    k("discord", "Discord", Caution, "Пропадёт голосовой чат. Если играешь с друзьями в голосе, не закрывай."),
    k("steam", "Steam", Caution, "Многим играм нужен запущенный Steam. Закрывай, только если игра не из Steam."),
    k("steamwebhelper", "Steam (встроенный браузер)", Caution, "Часть клиента Steam. Сильно грузит процессор, но закрытие может перезапустить клиент."),
    k("epicgameslauncher", "Epic Games Launcher", Caution, "Играм из Epic нужен запущенный лаунчер."),
    k("epicwebhelper", "Epic Games (встроенный браузер)", Caution, "Часть лаунчера Epic."),
    k("battle.net", "Battle.net", Caution, "Играм Blizzard нужен запущенный клиент."),
    k("eadesktop", "EA app", Caution, "Играм EA нужно приложение."),
    k("origin", "Origin", Caution, "Играм EA нужен запущенный клиент."),
    k("upc", "Ubisoft Connect", Caution, "Играм Ubisoft нужен запущенный клиент."),
    k("galaxyclient", "GOG Galaxy", Caution, "Нужен только для облачных сохранений и друзей."),
    k("obs64", "OBS Studio", Caution, "Прервётся запись или трансляция."),
    k("lghub", "Logitech G Hub", Caution, "Настройки мыши и клавиатуры могут сброситься."),
    k("lghub_agent", "Logitech G Hub", Caution, "Настройки мыши и клавиатуры могут сброситься."),
    k("synapse3", "Razer Synapse", Caution, "Настройки устройств Razer могут сброситься."),
    k("icue", "Corsair iCUE", Caution, "Подсветка и обороты вентиляторов могут сброситься."),
    k("nahimicsvc64", "Nahimic (звук)", Caution, "Пропадут звуковые эффекты, объёмный звук."),
    k("nvcontainer", "Службы NVIDIA", Caution, "Оверлей и запись NVIDIA перестанут работать."),
    k("vmmem", "Виртуальная машина / WSL", Caution, "Остановится WSL или виртуальная машина."),
    k("docker desktop", "Docker Desktop", Caution, "Остановятся контейнеры."),
    k("code", "VS Code", Caution, "Могут остаться несохранённые файлы."),
    // безопасно: фоновые программы, которые игре не нужны
    k("onedrive", "OneDrive", Safe, "Синхронизация файлов в фоне. Вернётся после перезагрузки или по запуску."),
    k("dropbox", "Dropbox", Safe, "Синхронизация файлов в фоне."),
    k("googledrivefs", "Google Диск", Safe, "Синхронизация файлов в фоне."),
    k("googleupdate", "Обновление Google", Safe, "Фоновое обновление браузера и программ Google."),
    k("microsoftedgeupdate", "Обновление Edge", Safe, "Фоновое обновление браузера."),
    k("spotify", "Spotify", Safe, "Музыка. Закрой, если слушаешь её не из Spotify."),
    k("telegram", "Telegram", Safe, "Мессенджер. Сообщения не пропадут, просто не будет уведомлений."),
    k("whatsapp", "WhatsApp", Safe, "Мессенджер."),
    k("viber", "Viber", Safe, "Мессенджер."),
    k("skype", "Skype", Safe, "Мессенджер."),
    k("teams", "Microsoft Teams", Safe, "Рабочий мессенджер."),
    k("ms-teams", "Microsoft Teams", Safe, "Рабочий мессенджер."),
    k("slack", "Slack", Safe, "Рабочий мессенджер."),
    k("zoom", "Zoom", Safe, "Видеозвонки."),
    k("yourphone", "Связь с телефоном", Safe, "Приложение «Связь с телефоном»."),
    k("phoneexperiencehost", "Связь с телефоном", Safe, "Приложение «Связь с телефоном»."),
    k("widgets", "Виджеты Windows", Safe, "Панель виджетов. Вернётся сама."),
    k("widgetservice", "Виджеты Windows", Safe, "Панель виджетов."),
    k("adobe desktop service", "Adobe Creative Cloud", Safe, "Фоновые службы Adobe."),
    k("creative cloud", "Adobe Creative Cloud", Safe, "Фоновые службы Adobe."),
    k("ccxprocess", "Adobe Creative Cloud", Safe, "Фоновые службы Adobe."),
    k("cclibrary", "Adobe Creative Cloud", Safe, "Фоновые службы Adobe."),
    k("adobeipcbroker", "Adobe", Safe, "Фоновые службы Adobe."),
    k("acrotray", "Adobe Acrobat", Safe, "Фоновый помощник Acrobat."),
    k("nvidia share", "NVIDIA Share", Safe, "Оверлей NVIDIA. Нужен только для записи и скриншотов NVIDIA."),
    k("nvsphelper64", "NVIDIA Share", Safe, "Оверлей NVIDIA."),
    k("hwinfo64", "HWiNFO", Safe, "Мониторинг. Закрой, если не смотришь датчики во время игры."),
    k("cpuz", "CPU-Z", Safe, "Информация о железе."),
    k("aida64", "AIDA64", Safe, "Мониторинг и тесты."),
    k("msteams", "Microsoft Teams", Safe, "Рабочий мессенджер."),
];

/// Находит описание по имени процесса (с «.exe» или без, любой регистр).
pub fn known(name: &str) -> Option<&'static Known> {
    let key = norm(name);
    KNOWN.iter().find(|x| x.key == key)
}

/// Имя в нижнем регистре без «.exe».
pub fn norm(name: &str) -> String {
    let n = name.trim().to_lowercase();
    n.strip_suffix(".exe").map(str::to_string).unwrap_or(n)
}

/// Класс процесса. `exe` — путь к exe, если Windows его отдала.
/// Если путь не виден, процесс принадлежит системе или запущен с повышенными правами: трогать не будем.
pub fn classify(name: &str, exe: Option<&str>) -> Class {
    if let Some(kn) = known(name) {
        return kn.class;
    }
    match exe {
        None => Never,
        Some(p) => {
            let p = p.to_lowercase().replace('/', "\\");
            // всё, что лежит в папке Windows или в защитных папках Microsoft, принадлежит системе
            if p.contains("\\windows\\system32\\")
                || p.contains("\\windows\\syswow64\\")
                || p.contains("\\windows\\systemapps\\")
                || p.contains("\\windows\\servicing\\")
                || p.contains("\\windows defender\\")
                || p.contains("\\windows\\winsxs\\")
                || p.contains("\\windows\\immersivecontrolpanel\\")
            {
                Never
            } else {
                Unknown
            }
        }
    }
}

/// Одна программа в списке: все её процессы вместе.
#[derive(Clone, Debug)]
pub struct Proc {
    pub name: String,
    pub count: usize,
    pub cpu: f32,
    pub mem_mb: u64,
    pub class: Class,
    pub known: Option<&'static Known>,
    pub exe: Option<String>,
}

pub fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing().with_cpu().with_memory().with_exe(UpdateKind::OnlyIfNotSet)
}

pub fn refresh(sys: &mut System) {
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind());
}

/// Процессы, сгруппированные по имени, самые прожорливые первыми.
pub fn list(sys: &System, cores: usize) -> Vec<Proc> {
    let mut by: HashMap<String, Proc> = HashMap::new();
    let cores = cores.max(1) as f32;
    for p in sys.processes().values() {
        let name = p.name().to_string_lossy().into_owned();
        if name == "System Idle Process" {
            continue;
        }
        let exe = p.exe().map(|e| e.to_string_lossy().into_owned());
        let e = by.entry(norm(&name)).or_insert_with(|| Proc {
            class: classify(&name, exe.as_deref()),
            known: known(&name),
            name: name.clone(),
            count: 0,
            cpu: 0.0,
            mem_mb: 0,
            exe: exe.clone(),
        });
        e.count += 1;
        e.cpu += p.cpu_usage() / cores;
        e.mem_mb += p.memory() / 1_048_576;
        if e.exe.is_none() {
            e.exe = exe;
        }
    }
    let mut v: Vec<Proc> = by.into_values().collect();
    v.sort_by(|a, b| {
        b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal).then(b.mem_mb.cmp(&a.mem_mb))
    });
    v
}

/// Закрывает все процессы с таким именем. Класс проверяется заново в момент закрытия:
/// «нельзя» не закрывается никогда, как бы ни вызвали. `keep_pid` (игра на переднем плане) не трогается.
/// Возвращает, сколько процессов закрыто, и путь к exe первого из них.
pub fn kill_by_name(sys: &System, name: &str, keep_pid: Option<u32>) -> (usize, Option<String>) {
    let key = norm(name);
    let mut killed = 0;
    let mut exe = None;
    for p in sys.processes().values() {
        let pname = p.name().to_string_lossy().into_owned();
        if norm(&pname) != key {
            continue;
        }
        let path = p.exe().map(|e| e.to_string_lossy().into_owned());
        if classify(&pname, path.as_deref()) == Never || Some(p.pid().as_u32()) == keep_pid {
            continue;
        }
        if p.pid().as_u32() == std::process::id() {
            continue;
        }
        if exe.is_none() {
            exe = path;
        }
        if p.kill() {
            killed += 1;
        }
    }
    (killed, exe)
}

/// Что игровой режим закрыл при входе (для возврата после игры и для показа в программе).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Stopped {
    pub name: String,
    pub exe: Option<String>,
    pub count: usize,
}

/// Закрывает программы из списка пользователя. Вызывается один раз при входе в игровой режим.
pub fn stop_for_game(names: &[String], keep_pid: Option<u32>) -> Vec<Stopped> {
    if names.is_empty() {
        return vec![];
    }
    let mut sys = System::new();
    refresh(&mut sys);
    let mut out = Vec::new();
    for n in names {
        let (count, exe) = kill_by_name(&sys, n, keep_pid);
        if count > 0 {
            out.push(Stopped { name: n.clone(), exe, count });
        }
    }
    out
}

/// Запускает закрытые программы обратно (без параметров: так, как будто пользователь открыл их сам).
pub fn restart(stopped: &[Stopped]) {
    for s in stopped {
        if let Some(exe) = &s.exe {
            let _ = std::process::Command::new(exe).spawn();
        }
    }
}

pub fn stopped_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("stopped.json")
}

pub fn save_stopped(v: &[Stopped]) {
    let _ = std::fs::create_dir_all(crate::paths::data_dir());
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(stopped_file(), s);
    }
}

pub fn load_stopped() -> Vec<Stopped> {
    std::fs::read_to_string(stopped_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_normalized() {
        assert_eq!(norm("Discord.exe"), "discord");
        assert_eq!(norm("  OBS64.EXE "), "obs64");
        assert_eq!(norm("battle.net.exe"), "battle.net");
        assert!(known("OneDrive.exe").is_some());
        assert!(known("nosuchapp.exe").is_none());
    }

    #[test]
    fn keys_are_unique_and_lowercase() {
        let mut seen = std::collections::HashSet::new();
        for x in KNOWN {
            assert_eq!(x.key, x.key.to_lowercase(), "{}", x.key);
            assert!(!x.key.ends_with(".exe"), "{}", x.key);
            assert!(seen.insert(x.key), "дубль {}", x.key);
            assert!(!x.title.is_empty() && !x.why.is_empty());
        }
    }

    #[test]
    fn critical_things_can_never_be_killed() {
        for n in ["csrss.exe", "winlogon.exe", "explorer.exe", "svchost.exe", "dwm.exe", "lsass.exe", "audiodg.exe",
                  "MsMpEng.exe", "RTSS.exe", "MSIAfterburner.exe", "EasyAntiCheat.exe", "BEService.exe", "vgc.exe", "GameLens.exe"] {
            assert_eq!(classify(n, Some("C:\\Anything\\x.exe")), Never, "{n}");
        }
    }

    #[test]
    fn unreadable_path_and_system_folders_are_protected() {
        assert_eq!(classify("mystery.exe", None), Never, "путь не виден: система или повышенные права");
        assert_eq!(classify("foo.exe", Some("C:\\Windows\\System32\\foo.exe")), Never);
        assert_eq!(classify("foo.exe", Some("c:/windows/syswow64/foo.exe")), Never);
        assert_eq!(classify("foo.exe", Some("C:\\Program Files\\Windows Defender\\foo.exe")), Never);
        assert_eq!(classify("foo.exe", Some("D:\\Apps\\foo.exe")), Unknown);
    }

    #[test]
    fn known_classes_apply() {
        assert_eq!(classify("OneDrive.exe", Some("C:\\x\\OneDrive.exe")), Safe);
        assert_eq!(classify("chrome.exe", Some("C:\\x\\chrome.exe")), Caution);
        assert_eq!(classify("Discord.exe", Some("C:\\x\\Discord.exe")), Caution);
        // точное совпадение: steam не задевает steamwebhelper и наоборот, оба «осторожно»
        assert_eq!(classify("steamwebhelper.exe", Some("C:\\x\\a.exe")), Caution);
    }

    #[test]
    fn nothing_to_stop_is_a_noop() {
        assert!(stop_for_game(&[], None).is_empty());
    }

    #[test]
    fn stopped_roundtrip() {
        let v = vec![Stopped { name: "onedrive.exe".into(), exe: Some("C:\\x\\OneDrive.exe".into()), count: 2 }];
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Stopped>>(&s).unwrap(), v);
    }

    /// Настоящее закрытие: копия `sleep` с уникальным именем, чтобы не задеть чужие процессы.
    #[cfg(unix)]
    #[test]
    fn kills_a_real_process_by_name_but_not_the_kept_pid() {
        let name = format!("glsleep{}", std::process::id());
        let bin = std::env::temp_dir().join(&name);
        let src = ["/bin/sleep", "/usr/bin/sleep"].into_iter().find(|p| std::path::Path::new(p).exists()).unwrap();
        std::fs::copy(src, &bin).unwrap();
        let mut child = std::process::Command::new(&bin).arg("60").spawn().unwrap();
        let pid = child.id();
        let mut sys = System::new();
        refresh(&mut sys);
        // с защитой этого pid ничего не закрывается
        assert_eq!(kill_by_name(&sys, &name, Some(pid)).0, 0);
        assert!(child.try_wait().unwrap().is_none(), "процесс жив");
        // без защиты закрывается
        let (n, exe) = kill_by_name(&sys, &name, None);
        assert_eq!(n, 1);
        assert!(exe.is_some());
        let status = child.wait().unwrap();
        assert!(!status.success(), "процесс завершён принудительно");
        let _ = std::fs::remove_file(bin);
    }
}
