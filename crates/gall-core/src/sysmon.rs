//! Live system stats for the sidebar monitor: CPU, GPU, memory, temperatures,
//! network and disk throughput. One `Monitor` keeps the previous sample so
//! usage and rates are deltas between calls (poll every 1–3 s).

use serde::Serialize;
use std::time::Instant;
use sysinfo::{Components, CpuRefreshKind, Networks, System};
#[cfg(not(target_os = "linux"))]
use sysinfo::{DiskRefreshKind, Disks};

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SysStats {
    pub host: String,
    pub os: String,
    pub cpu_model: String,
    pub cpu_cores: usize,
    /// 0–100
    pub cpu: f32,
    pub cpu_temp: Option<f32>,
    /// 0–100, when the GPU exposes it (Linux amdgpu/i915 sysfs; Windows GPU Engine
    /// performance counters, any vendor).
    pub gpu: Option<f32>,
    pub gpu_temp: Option<f32>,
    /// Dedicated GPU memory in use and in total, bytes (Windows: the GPU with the most
    /// memory of its own; Linux: amdgpu).
    pub gpu_mem_used: Option<u64>,
    pub gpu_mem_total: Option<u64>,
    pub mem_used: u64,
    pub mem_total: u64,
    /// Bytes per second since the previous sample.
    pub net_rx: f64,
    pub net_tx: f64,
    pub disk_read: Option<f64>,
    pub disk_write: Option<f64>,
}

pub struct Monitor {
    sys: Option<System>,
    nets: Option<Networks>,
    comps: Option<Components>,
    last: Instant,
    /// Cumulative bytes read/written at the previous sample.
    last_disk: Option<(u64, u64)>,
    /// Non-Linux: per-volume I/O counters from sysinfo (Windows: IOCTL_DISK_PERFORMANCE).
    #[cfg(not(target_os = "linux"))]
    disks: Option<Disks>,
    /// Windows: the GPU Engine performance counters (what Task Manager shows).
    #[cfg(windows)]
    gpu: Option<win_gpu::GpuCounters>,
    host: String,
    os: String,
    cpu_model: String,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Monitor {
    pub fn new() -> Self {
        let mut sys = System::new().ok();
        if let Some(s) = sys.as_mut() {
            s.refresh_cpu_list(CpuRefreshKind::everything());
            s.refresh_memory();
        }
        let cpu_model = sys
            .as_ref()
            .and_then(|s| s.cpus().first().map(|c| c.brand().trim().to_string()))
            .unwrap_or_default();
        let mut m = Monitor {
            sys,
            nets: Networks::new_with_refreshed_list().ok(),
            comps: Components::new_with_refreshed_list().ok(),
            last: Instant::now(),
            last_disk: None,
            #[cfg(not(target_os = "linux"))]
            disks: Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing()).ok(),
            #[cfg(windows)]
            gpu: win_gpu::GpuCounters::open(),
            host: System::host_name().unwrap_or_default(),
            os: System::long_os_version().unwrap_or_default(),
            cpu_model,
        };
        m.last_disk = m.disk_bytes();
        m
    }

    /// Cumulative bytes read and written by all disks, if the platform reports it.
    #[cfg(target_os = "linux")]
    fn disk_bytes(&mut self) -> Option<(u64, u64)> {
        disk_sectors().map(|(r, w)| (r * 512, w * 512))
    }

    #[cfg(not(target_os = "linux"))]
    fn disk_bytes(&mut self) -> Option<(u64, u64)> {
        let disks = self.disks.as_mut()?;
        disks.refresh_specifics(false, DiskRefreshKind::nothing().with_io_usage());
        // Volumes do not overlap, so summing them counts every byte once.
        let (r, w) = disks.iter().fold((0u64, 0u64), |(r, w), d| {
            let u = d.usage();
            (r + u.total_read_bytes, w + u.total_written_bytes)
        });
        (!disks.is_empty() && r + w > 0).then_some((r, w))
    }

    pub fn sample(&mut self) -> SysStats {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64().max(0.001);
        self.last = now;
        let mut st = SysStats {
            host: self.host.clone(),
            os: self.os.clone(),
            cpu_model: self.cpu_model.clone(),
            ..Default::default()
        };
        if let Some(s) = self.sys.as_mut() {
            s.refresh_cpu_usage();
            s.refresh_memory();
            st.cpu = s.global_cpu_usage().clamp(0.0, 100.0);
            st.cpu_cores = s.cpus().len();
            st.mem_used = s.used_memory();
            st.mem_total = s.total_memory();
        }
        if let Some(n) = self.nets.as_mut() {
            n.refresh(true);
            let (mut rx, mut tx) = (0u64, 0u64);
            for (name, d) in n.iter() {
                let name = name.to_string_lossy();
                if name == "lo"
                    || name.starts_with("docker")
                    || name.starts_with("veth")
                    || name.starts_with("virbr")
                    // Windows: loopback and Hyper-V/WSL switches mirror real traffic.
                    || name.starts_with("Loopback")
                    || name.starts_with("vEthernet")
                {
                    continue;
                }
                rx += d.received();
                tx += d.transmitted();
            }
            st.net_rx = rx as f64 / dt;
            st.net_tx = tx as f64 / dt;
        }
        if let Some(c) = self.comps.as_mut() {
            c.refresh(false);
            let temps: Vec<(String, f32)> = c
                .iter()
                .filter_map(|x| {
                    x.temperature()
                        .filter(|t| t.is_finite() && *t > 0.0)
                        .map(|t| (x.label().to_lowercase(), t))
                })
                .collect();
            st.cpu_temp = pick_temp(
                &temps,
                &[
                    "tctl",
                    "tdie",
                    "package id",
                    "k10temp",
                    "coretemp",
                    "cpu",
                    "soc_thermal",
                    "x86_pkg",
                ],
            );
            st.gpu_temp = pick_temp(&temps, &["amdgpu", "nouveau", "radeon", "nvidia", "gpu"]);
        }
        #[cfg(windows)]
        if let Some(g) = self.gpu.as_mut() {
            let s = g.sample();
            st.gpu = s.load;
            (st.gpu_mem_used, st.gpu_mem_total) = s.mem.unzip();
        }
        #[cfg(not(windows))]
        {
            st.gpu = gpu_busy();
            (st.gpu_mem_used, st.gpu_mem_total) = gpu_mem_sysfs().unzip();
        }
        let disk = self.disk_bytes();
        if let (Some((r0, w0)), Some((r1, w1))) = (self.last_disk, disk) {
            st.disk_read = Some(r1.saturating_sub(r0) as f64 / dt);
            st.disk_write = Some(w1.saturating_sub(w0) as f64 / dt);
        }
        self.last_disk = disk;
        st
    }
}

/// First sensor whose label contains one of `keys` (in priority order).
fn pick_temp(temps: &[(String, f32)], keys: &[&str]) -> Option<f32> {
    keys.iter()
        .find_map(|k| temps.iter().find(|(l, _)| l.contains(k)).map(|(_, t)| *t))
}

/// Busiest GPU engine from `\GPU Engine(*)\Utilization Percentage` samples, the way Task
/// Manager computes it: an engine's load is the sum over the processes using it
/// (instances `pid_…_luid_…_phys_0_eng_3_engtype_3D`), and the GPU shows its busiest engine.
#[cfg_attr(not(windows), allow(dead_code))]
fn busiest_engine<'a>(samples: impl IntoIterator<Item = (&'a str, f64)>) -> Option<f32> {
    let mut engines: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
    for (name, v) in samples {
        // The engine is everything from "luid_" up to the engine type.
        let Some(start) = name.find("luid_") else {
            continue;
        };
        let end = name.find("_engtype_").unwrap_or(name.len());
        if v.is_finite() && v > 0.0 && start < end {
            *engines.entry(&name[start..end]).or_default() += v;
        }
    }
    let have_any = !engines.is_empty();
    let max = engines.into_values().fold(0.0f64, f64::max);
    // No instance with load still means a GPU at 0 % (counters exist); None only when
    // the caller found no counters at all.
    Some(if have_any { max.min(100.0) as f32 } else { 0.0 })
}

/// The `luid_…` part of a GPU counter instance name for an adapter LUID, lower-case
/// (`pid_…_luid_0x00000000_0x00014BEF_phys_0_…`).
#[cfg_attr(not(windows), allow(dead_code))]
fn luid_key(high: i32, low: u32) -> String {
    format!("luid_0x{:08x}_0x{:08x}", high as u32, low)
}

/// Windows GPU load and memory through the Performance Data Helper (pdh.dll) and the
/// adapter list from DXGI: no admin rights, no vendor tools, any vendor.
#[cfg(windows)]
mod win_gpu {
    use std::ffi::c_void;

    #[link(name = "pdh")]
    extern "system" {
        fn PdhOpenQueryW(source: *const u16, user: usize, query: *mut isize) -> u32;
        fn PdhAddEnglishCounterW(
            query: isize,
            path: *const u16,
            user: usize,
            counter: *mut isize,
        ) -> u32;
        fn PdhCollectQueryData(query: isize) -> u32;
        fn PdhGetFormattedCounterArrayW(
            counter: isize,
            format: u32,
            size: *mut u32,
            count: *mut u32,
            items: *mut Item,
        ) -> u32;
        fn PdhCloseQuery(query: isize) -> u32;
    }
    #[link(name = "dxgi")]
    extern "system" {
        fn CreateDXGIFactory1(riid: *const Guid, factory: *mut *mut c_void) -> i32;
    }
    const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
    const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
    const PDH_MORE_DATA: u32 = 0x8000_07D2;
    const PDH_CSTATUS_VALID_DATA: u32 = 0;
    const PDH_CSTATUS_NEW_DATA: u32 = 1;
    /// A card needs this much memory of its own to count as the dedicated GPU.
    const MIN_DEDICATED: u64 = 512 << 20;

    /// PDH_FMT_COUNTERVALUE_ITEM_W with a double value.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Item {
        name: *const u16,
        status: u32,
        value: f64,
    }

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const IID_IDXGI_FACTORY1: Guid = Guid(
        0x770a_ae78,
        0xf26f,
        0x4dba,
        [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
    );

    /// DXGI_ADAPTER_DESC1.
    #[repr(C)]
    struct AdapterDesc1 {
        description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        sub_sys_id: u32,
        revision: u32,
        dedicated_video_memory: usize,
        dedicated_system_memory: usize,
        shared_system_memory: usize,
        luid_low: u32,
        luid_high: i32,
        flags: u32,
    }
    const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;

    /// COM method `index` of the interface at `obj` (vtable is the first pointer).
    unsafe fn method<F: Copy>(obj: *mut c_void, index: usize) -> F {
        let vtbl = *(obj as *const *const usize);
        std::mem::transmute_copy(&*vtbl.add(index))
    }

    /// Hardware adapters: (LUID key, memory of its own in bytes).
    fn adapters() -> Vec<(String, u64)> {
        type EnumAdapters1 = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
        type GetDesc1 = unsafe extern "system" fn(*mut c_void, *mut AdapterDesc1) -> i32;
        type Release = unsafe extern "system" fn(*mut c_void) -> u32;
        let mut out = Vec::new();
        // SAFETY: COM calls through the documented vtable slots (IDXGIFactory1::EnumAdapters1
        // = 12, IDXGIAdapter1::GetDesc1 = 10, IUnknown::Release = 2); every object is released.
        unsafe {
            let mut factory: *mut c_void = std::ptr::null_mut();
            if CreateDXGIFactory1(&IID_IDXGI_FACTORY1, &mut factory) < 0 || factory.is_null() {
                return out;
            }
            for i in 0.. {
                let mut adapter: *mut c_void = std::ptr::null_mut();
                if method::<EnumAdapters1>(factory, 12)(factory, i, &mut adapter) < 0
                    || adapter.is_null()
                {
                    break; // DXGI_ERROR_NOT_FOUND: no more adapters
                }
                let mut d: AdapterDesc1 = std::mem::zeroed();
                if method::<GetDesc1>(adapter, 10)(adapter, &mut d) >= 0
                    && d.flags & DXGI_ADAPTER_FLAG_SOFTWARE == 0
                {
                    out.push((
                        super::luid_key(d.luid_high, d.luid_low),
                        d.dedicated_video_memory as u64,
                    ));
                }
                method::<Release>(adapter, 2)(adapter);
            }
            method::<Release>(factory, 2)(factory);
        }
        out
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// An open query; handles are plain integers so the monitor stays `Send`.
    pub struct GpuCounters {
        query: isize,
        engines: isize,
        memory: isize,
        /// The dedicated GPU (most memory of its own): its LUID key and memory size.
        main: Option<(String, u64)>,
    }

    pub struct GpuSample {
        pub load: Option<f32>,
        /// Dedicated memory in use and in total, for the dedicated GPU.
        pub mem: Option<(u64, u64)>,
    }

    impl GpuCounters {
        /// None when the counters don't exist (no WDDM 2 driver, some VMs).
        pub fn open() -> Option<Self> {
            let (mut query, mut engines, mut memory) = (0isize, 0isize, 0isize);
            // SAFETY: out-pointers to locals, NUL-terminated paths; the query is closed in Drop.
            unsafe {
                if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                    return None;
                }
                if PdhAddEnglishCounterW(
                    query,
                    wide(r"\GPU Engine(*)\Utilization Percentage").as_ptr(),
                    0,
                    &mut engines,
                ) != 0
                {
                    PdhCloseQuery(query);
                    return None;
                }
                if PdhAddEnglishCounterW(
                    query,
                    wide(r"\GPU Adapter Memory(*)\Dedicated Usage").as_ptr(),
                    0,
                    &mut memory,
                ) != 0
                {
                    memory = 0;
                }
                // Rates need two samples: take the first now.
                PdhCollectQueryData(query);
            }
            let main = adapters()
                .into_iter()
                .filter(|(_, m)| *m >= MIN_DEDICATED)
                .max_by_key(|(_, m)| *m);
            Some(GpuCounters {
                query,
                engines,
                memory,
                main,
            })
        }

        /// Instance names and values of one counter, as of the last collection.
        unsafe fn values(counter: isize) -> Option<Vec<(String, f64)>> {
            let format = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
            let (mut size, mut count) = (0u32, 0u32);
            let r = PdhGetFormattedCounterArrayW(
                counter,
                format,
                &mut size,
                &mut count,
                std::ptr::null_mut(),
            );
            if r != PDH_MORE_DATA || size == 0 {
                return None;
            }
            // u64 words keep the items 8-byte aligned; names point into the same buffer.
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr() as *mut Item;
            if PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, items) != 0 {
                return None;
            }
            Some(
                std::slice::from_raw_parts(items, count as usize)
                    .iter()
                    .filter(|i| {
                        matches!(i.status, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA)
                            && !i.name.is_null()
                    })
                    .map(|i| {
                        let len = (0..).take_while(|&k| *i.name.add(k) != 0).count();
                        (
                            String::from_utf16_lossy(std::slice::from_raw_parts(i.name, len))
                                .to_lowercase(),
                            i.value,
                        )
                    })
                    .collect(),
            )
        }

        /// Load of the busiest engine since the previous call, and memory in use, for the
        /// dedicated GPU (or all GPUs when there is none).
        pub fn sample(&mut self) -> GpuSample {
            let mut out = GpuSample {
                load: None,
                mem: None,
            };
            // SAFETY: see `values`; the query and counters stay open until Drop.
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return out;
                }
                let main = self.main.as_ref().map(|(k, _)| k.as_str());
                let ours = |n: &str| main.is_none_or(|k| n.contains(k));
                if let Some(v) = Self::values(self.engines) {
                    out.load = super::busiest_engine(
                        v.iter()
                            .filter(|(n, _)| ours(n))
                            .map(|(n, x)| (n.as_str(), *x)),
                    );
                }
                if let (Some((key, total)), true) = (self.main.as_ref(), self.memory != 0) {
                    if let Some(v) = Self::values(self.memory) {
                        let used: f64 = v
                            .iter()
                            .filter(|(n, _)| n.contains(key.as_str()))
                            .map(|(_, x)| *x)
                            .sum();
                        out.mem = Some((used as u64, *total));
                    }
                }
            }
            out
        }
    }

    impl Drop for GpuCounters {
        fn drop(&mut self) {
            // SAFETY: the query was opened by PdhOpenQueryW and is closed once.
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }
}

/// Dedicated GPU memory in use and in total (Linux: amdgpu's `mem_info_vram_*`; the card
/// with the most memory of its own).
#[cfg_attr(windows, allow(dead_code))]
fn gpu_mem_sysfs() -> Option<(u64, u64)> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).ok()?.trim().parse::<u64>().ok();
    std::fs::read_dir("/sys/class/drm")
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("card") && !n.contains('-')
        })
        .filter_map(|e| {
            let dev = e.path().join("device");
            Some((
                read(dev.join("mem_info_vram_used"))?,
                read(dev.join("mem_info_vram_total"))?,
            ))
        })
        .filter(|(_, total)| *total > 0)
        .max_by_key(|(_, total)| *total)
}
/// GPU busy % from sysfs (amdgpu, recent i915/xe expose `gpu_busy_percent`).
#[cfg_attr(windows, allow(dead_code))]
fn gpu_busy() -> Option<f32> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let rd = std::fs::read_dir("/sys/class/drm").ok()?;
    let mut best: Option<f32> = None;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        if let Ok(s) = std::fs::read_to_string(e.path().join("device/gpu_busy_percent")) {
            if let Ok(v) = s.trim().parse::<f32>() {
                best = Some(best.map_or(v, |b: f32| b.max(v)));
            }
        }
    }
    best
}

/// Total sectors read/written by whole disks (Linux /proc/diskstats). Partitions,
/// loop, ram/zram and device-mapper devices are skipped so nothing is counted twice.
#[cfg(target_os = "linux")]
fn disk_sectors() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/diskstats").ok()?;
    Some(parse_diskstats(&text, |n| {
        std::path::Path::new("/sys/block").join(n).exists()
    }))
}

#[cfg(any(target_os = "linux", test))]
fn parse_diskstats(text: &str, is_whole_disk: impl Fn(&str) -> bool) -> (u64, u64) {
    let (mut r, mut w) = (0u64, 0u64);
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 {
            continue;
        }
        let name = f[2];
        if ["loop", "ram", "zram", "dm-", "sr", "md", "fd"]
            .iter()
            .any(|p| name.starts_with(p))
            || !is_whole_disk(name)
        {
            continue;
        }
        r += f[5].parse::<u64>().unwrap_or(0);
        w += f[9].parse::<u64>().unwrap_or(0);
    }
    (r, w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_load_is_the_busiest_engine_summed_over_processes() {
        let s = [
            (
                "pid_100_luid_0x00000000_0x0000D1B3_phys_0_eng_0_engtype_3D",
                20.0,
            ),
            (
                "pid_200_luid_0x00000000_0x0000D1B3_phys_0_eng_0_engtype_3D",
                15.5,
            ),
            (
                "pid_200_luid_0x00000000_0x0000D1B3_phys_0_eng_3_engtype_VideoDecode",
                30.0,
            ),
            // Another adapter (iGPU) is not added to the first.
            (
                "pid_300_luid_0x00000000_0x0000AAAA_phys_0_eng_0_engtype_3D",
                25.0,
            ),
            (
                "pid_400_luid_0x00000000_0x0000D1B3_phys_0_eng_1_engtype_Copy",
                f64::NAN,
            ),
        ];
        assert_eq!(busiest_engine(s), Some(35.5));
        assert_eq!(
            busiest_engine([
                ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 80.0),
                ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 70.0)
            ]),
            Some(100.0)
        );
        assert_eq!(busiest_engine([]), Some(0.0), "counters but idle: 0 %");
        // Counter instances name the adapter by LUID (high part first), lower-cased here.
        assert_eq!(luid_key(0, 0x14BEF), "luid_0x00000000_0x00014bef");
    }

    /// This computer's reading: `cargo test -p gall-core live_gpu -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_gpu() {
        let mut m = Monitor::new();
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let s = m.sample();
            println!(
                "gpu {:?}  mem {:?} / {:?}  cpu {:.1}  cpu temp {:?}  gpu temp {:?}",
                s.gpu, s.gpu_mem_used, s.gpu_mem_total, s.cpu, s.cpu_temp, s.gpu_temp
            );
        }
    }

    #[test]
    fn diskstats_counts_whole_disks_only() {
        let t = "\
 259       0 nvme0n1 100 0 1000 0 50 0 2000 0 0 0 0
 259       1 nvme0n1p1 10 0 100 0 5 0 200 0 0 0 0
   7       0 loop0 9 0 999 0 0 0 999 0 0 0 0
 251       0 zram0 1 0 50 0 1 0 50 0 0 0 0
 253       0 dm-0 1 0 700 0 1 0 700 0 0 0 0
   8       0 sda 1 0 24 0 1 0 8 0 0 0 0";
        let whole = |n: &str| !n.contains('p') || n == "loop0";
        assert_eq!(parse_diskstats(t, whole), (1024, 2008));
    }

    #[test]
    fn temperature_priority() {
        let t = vec![
            ("nvme composite".to_string(), 40.0),
            ("k10temp tctl".to_string(), 55.0),
            ("amdgpu edge".to_string(), 48.0),
        ];
        assert_eq!(pick_temp(&t, &["tctl", "cpu"]), Some(55.0));
        assert_eq!(pick_temp(&t, &["amdgpu"]), Some(48.0));
        assert_eq!(pick_temp(&t, &["coretemp"]), None);
    }

    #[test]
    fn sampling_works() {
        let mut m = Monitor::new();
        let s = m.sample();
        assert!(s.cpu >= 0.0 && s.cpu <= 100.0);
        assert!(s.mem_total == 0 || s.mem_used <= s.mem_total);
    }

    /// Disk throughput is reported on Linux (/proc/diskstats) and Windows (sysinfo).
    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn disk_io_is_reported() {
        let mut m = Monitor::new();
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), vec![7u8; 4 << 20]).unwrap();
        let _ = f.as_file().sync_all();
        let s = m.sample();
        eprintln!(
            "disk read {:?} B/s, write {:?} B/s",
            s.disk_read, s.disk_write
        );
        assert!(s.disk_read.is_some() && s.disk_write.is_some());
    }
}
