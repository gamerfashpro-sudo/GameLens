//! Надпись поверх игры в ЛЮБОМ режиме (в том числе в настоящем полноэкранном) через RivaTuner Statistics Server.
//!
//! Обычное окно Windows не может рисовать поверх игры в эксклюзивном полноэкранном режиме. RTSS (он ставится
//! вместе с MSI Afterburner) умеет, потому что встроен в игру сам, и его хук давно известен античитам.
//! Мы ничего в игру не внедряем: просто пишем строку в общую память RTSS («RTSSSharedMemoryV2»), как это делают
//! HWiNFO, AIDA64 и другие мониторы, а рисует её RTSS. Лишней нагрузки нет: это несколько байт раз в полсекунды.
//!
//! Формат общей памяти читается из её собственного заголовка (размеры записей, смещения), а не зашит в код;
//! при любом несоответствии (подпись, версия, выход за границы памяти) мы ничего не пишем.

use std::sync::atomic::{AtomicU32, Ordering};

/// Имя общей памяти RTSS.
#[allow(dead_code)]
pub const MAP_NAME: &[u8] = b"RTSSSharedMemoryV2\0";

const OWNER: &str = "GameLens";
const OFF_SIGNATURE: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_OSD_ENTRY_SIZE: usize = 20;
const OFF_OSD_ARR_OFFSET: usize = 24;
const OFF_OSD_ARR_SIZE: usize = 28;
const OFF_OSD_FRAME: usize = 32;
const OFF_BUSY: usize = 36;
const HEADER_MIN: usize = 40;
/// поле szOSD (текст, 256 байт) и szOSDOwner (владелец, 256 байт) идут друг за другом в записи
const OSD_LEN: usize = 256;
const OWNER_OFF: usize = 256;
const OSD_EX_OFF: usize = 512;
const OSD_EX_LEN: usize = 4096;
const VER_OSD_EX: u32 = 0x0002_0007;
const VER_BUSY: u32 = 0x0002_000E;

/// Окно на общую память RTSS (или на любой буфер для проверок).
pub struct Shm {
    base: *mut u8,
    len: usize,
    slot: Option<usize>,
}

// Указатель смотрит в память, которой владеет отображение файла; пользуемся ею из одного потока.
unsafe impl Send for Shm {}

impl Shm {
    /// # Safety
    /// `base` должен указывать на `len` доступных для записи байт на всё время жизни `Shm`.
    pub unsafe fn new(base: *mut u8, len: usize) -> Shm {
        Shm { base, len, slot: None }
    }

    fn rd(&self, off: usize) -> u32 {
        if off + 4 > self.len {
            return 0;
        }
        unsafe { std::ptr::read_unaligned(self.base.add(off) as *const u32) }
    }

    /// Заголовок похож на настоящий и записи лежат внутри памяти.
    pub fn valid(&self) -> bool {
        if self.len < HEADER_MIN {
            return false;
        }
        let sig = self.rd(OFF_SIGNATURE);
        // 'RTSS' в памяти может лежать в любом порядке байт, зависит от сборки: принимаем оба
        if sig != u32::from_le_bytes(*b"RTSS") && sig != u32::from_be_bytes(*b"RTSS") {
            return false;
        }
        if self.rd(OFF_VERSION) < 0x0002_0000 {
            return false;
        }
        let entry = self.rd(OFF_OSD_ENTRY_SIZE) as usize;
        let off = self.rd(OFF_OSD_ARR_OFFSET) as usize;
        let n = self.rd(OFF_OSD_ARR_SIZE) as usize;
        if entry < OSD_LEN * 2 || n == 0 || n > 64 {
            return false;
        }
        off >= HEADER_MIN && off.checked_add(entry.saturating_mul(n)).is_some_and(|end| end <= self.len)
    }

    fn entry_off(&self, i: usize) -> usize {
        self.rd(OFF_OSD_ARR_OFFSET) as usize + i * self.rd(OFF_OSD_ENTRY_SIZE) as usize
    }

    fn owner_at(&self, i: usize) -> Vec<u8> {
        let o = self.entry_off(i) + OWNER_OFF;
        (0..OSD_LEN).map(|k| unsafe { *self.base.add(o + k) }).take_while(|b| *b != 0).collect()
    }

    /// Занимает свободную запись (запись 0 принадлежит основным клиентам RTSS и не трогается).
    fn claim(&mut self) -> Option<usize> {
        if let Some(s) = self.slot {
            return Some(s);
        }
        let n = self.rd(OFF_OSD_ARR_SIZE) as usize;
        for i in 1..n {
            let who = self.owner_at(i);
            if who.is_empty() || who == OWNER.as_bytes() {
                let o = self.entry_off(i) + OWNER_OFF;
                unsafe {
                    std::ptr::write_bytes(self.base.add(o), 0, OSD_LEN);
                    std::ptr::copy_nonoverlapping(OWNER.as_ptr(), self.base.add(o), OWNER.len());
                }
                self.slot = Some(i);
                return Some(i);
            }
        }
        None
    }

    /// Блокировка записи для RTSS версии 2.14+ (бит 0 поля dwBusy). Ждём недолго и не зависаем.
    fn lock(&self) -> bool {
        if self.rd(OFF_VERSION) < VER_BUSY {
            return true;
        }
        let busy = unsafe { &*(self.base.add(OFF_BUSY) as *const AtomicU32) };
        for _ in 0..200 {
            if busy.fetch_or(1, Ordering::Acquire) & 1 == 0 {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        false
    }

    fn unlock(&self) {
        if self.rd(OFF_VERSION) >= VER_BUSY {
            let busy = unsafe { &*(self.base.add(OFF_BUSY) as *const AtomicU32) };
            busy.fetch_and(!1, Ordering::Release);
        }
    }

    fn bump_frame(&self) {
        let f = unsafe { &*(self.base.add(OFF_OSD_FRAME) as *const AtomicU32) };
        f.fetch_add(1, Ordering::Release);
    }

    /// Пишет строку в свою запись. Пустая строка стирает текст, и RTSS перестаёт его показывать.
    /// Возвращает false, если память не подошла или запись занята.
    pub fn set_text(&mut self, text: &str) -> bool {
        if !self.valid() {
            return false;
        }
        let Some(i) = self.claim() else { return false };
        let ascii = ascii_only(text);
        let entry = self.entry_off(i);
        let entry_size = self.rd(OFF_OSD_ENTRY_SIZE) as usize;
        if !self.lock() {
            return false;
        }
        unsafe {
            let short = ascii.as_bytes().len().min(OSD_LEN - 1);
            std::ptr::write_bytes(self.base.add(entry), 0, OSD_LEN);
            std::ptr::copy_nonoverlapping(ascii.as_ptr(), self.base.add(entry), short);
            if self.rd(OFF_VERSION) >= VER_OSD_EX && entry_size >= OSD_EX_OFF + OSD_EX_LEN {
                let long = ascii.as_bytes().len().min(OSD_EX_LEN - 1);
                std::ptr::write_bytes(self.base.add(entry + OSD_EX_OFF), 0, OSD_EX_LEN);
                std::ptr::copy_nonoverlapping(ascii.as_ptr(), self.base.add(entry + OSD_EX_OFF), long);
            }
        }
        self.bump_frame();
        self.unlock();
        true
    }

    /// Отдаёт запись обратно RTSS: стирает текст и имя владельца.
    pub fn release(&mut self) {
        if let Some(i) = self.slot.take() {
            if !self.valid() || !self.lock() {
                return;
            }
            let entry = self.entry_off(i);
            let entry_size = self.rd(OFF_OSD_ENTRY_SIZE) as usize;
            unsafe {
                std::ptr::write_bytes(self.base.add(entry), 0, OSD_LEN * 2);
                if entry_size >= OSD_EX_OFF + OSD_EX_LEN {
                    std::ptr::write_bytes(self.base.add(entry + OSD_EX_OFF), 0, OSD_EX_LEN);
                }
            }
            self.bump_frame();
            self.unlock();
        }
    }
}

/// RTSS читает однобайтовую кодовую страницу: кириллицу и прочее заменяем на «?», чтобы не вышел мусор.
fn ascii_only(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '?' }).collect()
}

/// Подключение к RTSS: открывает общую память, пока RTSS запущен.
pub struct Rtss {
    shm: Shm,
    #[cfg(any(windows, gl_check))]
    handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(any(windows, gl_check))]
    view: *mut core::ffi::c_void,
    last: String,
}

unsafe impl Send for Rtss {}

impl Rtss {
    /// None, если RTSS не запущен или формат памяти не подошёл.
    pub fn connect() -> Option<Rtss> {
        #[cfg(any(windows, gl_check))]
        unsafe {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::Memory::*;
            let h = OpenFileMappingA(FILE_MAP_ALL_ACCESS, 0, MAP_NAME.as_ptr());
            if h.is_null() {
                return None;
            }
            let view = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                CloseHandle(h);
                return None;
            }
            let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            let got = VirtualQuery(view.Value, &mut mbi, std::mem::size_of::<MEMORY_BASIC_INFORMATION>());
            let len = if got == 0 { 0 } else { mbi.RegionSize };
            let shm = Shm::new(view.Value as *mut u8, len);
            if !shm.valid() {
                UnmapViewOfFile(view);
                CloseHandle(h);
                return None;
            }
            Some(Rtss { shm, handle: h, view: view.Value, last: String::new() })
        }
        #[cfg(not(any(windows, gl_check)))]
        {
            None
        }
    }

    /// Показывает строку; в RTSS пишет только при изменении. false — RTSS пропал или память не подошла.
    pub fn show(&mut self, text: &str) -> bool {
        if self.last == text {
            return self.shm.valid();
        }
        let ok = self.shm.set_text(text);
        if ok {
            self.last = text.to_string();
        }
        ok
    }
}

impl Drop for Rtss {
    fn drop(&mut self) {
        self.shm.release();
        #[cfg(any(windows, gl_check))]
        unsafe {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::Memory::*;
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view });
            CloseHandle(self.handle);
        }
    }
}

/// Запущен ли RTSS сейчас (для подсказки в настройках).
pub fn available() -> bool {
    Rtss::connect().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Собирает поддельную общую память RTSS нужной версии: заголовок + 3 записи OSD.
    fn fake(version: u32, entry: usize, n: usize) -> Vec<u8> {
        let off = 256usize;
        let mut m = vec![0u8; off + entry * n];
        let w = |m: &mut Vec<u8>, at: usize, v: u32| m[at..at + 4].copy_from_slice(&v.to_le_bytes());
        w(&mut m, OFF_SIGNATURE, u32::from_le_bytes(*b"RTSS"));
        w(&mut m, OFF_VERSION, version);
        w(&mut m, OFF_OSD_ENTRY_SIZE, entry as u32);
        w(&mut m, OFF_OSD_ARR_OFFSET, off as u32);
        w(&mut m, OFF_OSD_ARR_SIZE, n as u32);
        m
    }

    fn cstr(m: &[u8], at: usize) -> String {
        let end = m[at..].iter().position(|b| *b == 0).unwrap_or(0);
        String::from_utf8_lossy(&m[at..at + end]).to_string()
    }

    #[test]
    fn rejects_garbage() {
        let mut m = vec![0u8; 4096];
        let s = unsafe { Shm::new(m.as_mut_ptr(), m.len()) };
        assert!(!s.valid());
        let mut tiny = vec![0u8; 8];
        assert!(!unsafe { Shm::new(tiny.as_mut_ptr(), 8) }.valid());
    }

    #[test]
    fn rejects_entries_past_the_end() {
        let mut m = fake(0x0002_000E, 4864, 8);
        m.truncate(3000); // массив записей не помещается
        let len = m.len();
        assert!(!unsafe { Shm::new(m.as_mut_ptr(), len) }.valid());
    }

    #[test]
    fn writes_into_free_slot_not_zero() {
        let entry = 256 + 256 + 4096;
        let mut m = fake(0x0002_000E, entry, 4);
        let len = m.len();
        let mut s = unsafe { Shm::new(m.as_mut_ptr(), len) };
        assert!(s.set_text("60 FPS"));
        let base = 256 + entry; // запись 1
        assert_eq!(cstr(&m, base), "60 FPS");
        assert_eq!(cstr(&m, base + OWNER_OFF), "GameLens");
        assert_eq!(cstr(&m, base + OSD_EX_OFF), "60 FPS");
        assert_eq!(cstr(&m, 256), "", "запись 0 не тронута");
        let frame = u32::from_le_bytes(m[OFF_OSD_FRAME..OFF_OSD_FRAME + 4].try_into().unwrap());
        assert_eq!(frame, 1, "счётчик кадров увеличен, чтобы RTSS перерисовал");
        let busy = u32::from_le_bytes(m[OFF_BUSY..OFF_BUSY + 4].try_into().unwrap());
        assert_eq!(busy & 1, 0, "блокировка снята");
    }

    #[test]
    fn skips_slots_owned_by_others_and_reuses_own() {
        let entry = 512;
        let mut m = fake(0x0002_0005, entry, 4);
        let at = 256 + entry + OWNER_OFF; // запись 1 занята другой программой
        m[at..at + 5].copy_from_slice(b"HWiNF");
        let len = m.len();
        let mut s = unsafe { Shm::new(m.as_mut_ptr(), len) };
        assert!(s.set_text("1"));
        assert_eq!(cstr(&m, 256 + 2 * entry + OWNER_OFF), "GameLens");
        assert_eq!(cstr(&m, 256 + entry + OWNER_OFF), "HWiNF", "чужая запись цела");
    }

    #[test]
    fn no_free_slot_means_no_write() {
        let entry = 512;
        let mut m = fake(0x0002_0005, entry, 2); // есть только запись 0 и одна запись 1
        let at = 256 + entry + OWNER_OFF;
        m[at] = b'X';
        let len = m.len();
        let mut s = unsafe { Shm::new(m.as_mut_ptr(), len) };
        assert!(!s.set_text("1"));
    }

    #[test]
    fn release_clears_and_non_ascii_is_masked() {
        let entry = 512;
        let mut m = fake(0x0002_0005, entry, 3);
        let len = m.len();
        let mut s = unsafe { Shm::new(m.as_mut_ptr(), len) };
        assert!(s.set_text("FPS: нужны"));
        assert_eq!(cstr(&m, 256 + entry), "FPS: ?????");
        s.release();
        assert_eq!(cstr(&m, 256 + entry), "");
        assert_eq!(cstr(&m, 256 + entry + OWNER_OFF), "");
    }

    #[test]
    fn long_text_is_cut_not_overflowed() {
        let entry = 512;
        let mut m = fake(0x0002_0005, entry, 3);
        let len = m.len();
        let mut s = unsafe { Shm::new(m.as_mut_ptr(), len) };
        assert!(s.set_text(&"a".repeat(1000)));
        assert_eq!(cstr(&m, 256 + entry).len(), 255);
        assert_eq!(cstr(&m, 256 + entry + OWNER_OFF), "GameLens", "владелец не затёрт");
    }
}
