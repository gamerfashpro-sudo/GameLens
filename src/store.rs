use crate::model::Sample;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Запись: одна строка JSON = один замер. Ничего не держим в памяти,
/// история живёт только на диске — поэтому рекордер весит считанные МБ.
pub struct Writer {
    out: BufWriter<File>,
    pending: u32,
}

impl Writer {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let f = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { out: BufWriter::with_capacity(16 * 1024, f), pending: 0 })
    }

    pub fn write(&mut self, s: &Sample) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.out, s)?;
        self.out.write_all(b"\n")?;
        self.pending += 1;
        // сбрасываем на диск раз в 5 замеров: при падении игры/ПК теряем максимум несколько секунд
        if self.pending >= 5 {
            self.out.flush()?;
            self.pending = 0;
        }
        Ok(())
    }
}

pub fn read_all(path: &Path) -> std::io::Result<Vec<Sample>> {
    let f = File::open(path)?;
    let mut v = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // последняя строка может быть оборвана при внезапном выключении — пропускаем битые
        if let Ok(s) = serde_json::from_str::<Sample>(&line) {
            v.push(s);
        }
    }
    Ok(v)
}

/// Оставляем только `keep` самых новых сессий, чтобы папка не росла бесконечно.
pub fn prune(dir: &Path, keep: usize) -> std::io::Result<usize> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false))
        .collect();
    files.sort();
    let mut removed = 0;
    while files.len() > keep {
        fs::remove_file(files.remove(0))?;
        removed += 1;
    }
    Ok(removed)
}
