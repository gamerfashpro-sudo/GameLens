use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub fn label(self) -> &'static str {
        match self {
            Corner::TopLeft => "Слева сверху",
            Corner::TopRight => "Справа сверху",
            Corner::BottomLeft => "Слева снизу",
            Corner::BottomRight => "Справа снизу",
        }
    }
}

/// Настройки программы (файл config.json в папке данных).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// показывать FPS поверх игры в игровом режиме
    pub fps_overlay: bool,
    pub corner: Corner,
    /// непрозрачность надписи, % (30..100). Меньше — мягче и менее заметно
    pub opacity: u8,
    /// высота шрифта, px
    pub size: u8,
}

impl Default for Config {
    fn default() -> Self {
        Config { fps_overlay: true, corner: Corner::TopLeft, opacity: 80, size: 22 }
    }
}

impl Config {
    pub fn load() -> Config {
        std::fs::read_to_string(crate::paths::config_file())
            .ok()
            .and_then(|s| serde_json::from_str::<Config>(&s).ok())
            .map(|mut c| {
                c.opacity = c.opacity.clamp(30, 100);
                c.size = c.size.clamp(14, 40);
                c
            })
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::paths::data_dir());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(crate::paths::config_file(), s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_json() {
        let c: Config = serde_json::from_str(r#"{"opacity": 55}"#).unwrap();
        assert_eq!(c.opacity, 55);
        assert_eq!(c.corner, Corner::TopLeft);
        assert!(c.fps_overlay);
    }

    #[test]
    fn roundtrip() {
        let c = Config { corner: Corner::BottomRight, ..Config::default() };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&s).unwrap(), c);
    }
}
