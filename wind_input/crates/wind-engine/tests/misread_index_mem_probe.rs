//! 误读自动推断要的「词 → 读音」索引（拼音方案的 `.wridx` 反查索引）的内存探针（`#[ignore]`，手动跑）。
//!
//! 三档、一档一进程（同进程里前一档 free 的页被 glibc 留着复用，差值读出来是 0）：
//! - `base`：只载拼音引擎、打几串，作基线；
//! - `build`：删掉缓存再建索引 ⇒ 构建期峰值；
//! - `reuse`：缓存在盘上 ⇒ 直接开（大索引走 mmap），查一遍 ⇒ 稳态常驻。
//!
//! 「常驻」取 `RssAnon`（≈ Windows Private，mmap 的文件页不计）与计数分配器 live 两种口径；
//! 「峰值」取计数分配器的 peak 与 `VmHWM`。数字随机器变，是探针不是判据。
//!
//! ```text
//! cd wind_input
//! cargo test -p wind-engine --release --test misread_index_mem_probe -- --ignored --nocapture
//! ```
//!
//! 缓存根经 `XDG_DATA_HOME` 指到临时目录，不碰真实缓存；词库在 `build_dev/data`。
//!
//! 实测（本机，64 万词，`.wridx` 24.1 MB，基线 RssAnon 2.0 MB / VmHWM 18.8 MB）：
//!
//! | 档 | 增量 |
//! |---|---|
//! | 稳态（缓存在盘上，mmap 打开） | RssAnon +0.01 MB，2 ms |
//! | 首建峰值 | 堆 peak +117.6 MB，VmHWM +163 MB，0.8 s |
//! | 首建后高水位（malloc_trim 后） | RssAnon +0 ~ +22 MB（glibc 留着不还） |
//!
//! 峰值只在首建 / 词库变化后出现，故读音索引放后台线程建，未就绪时不标误读。

// 读 `/proc`、调 glibc 的 `malloc_trim`：只在 Linux glibc 上编。
#![cfg(all(target_os = "linux", target_env = "gnu"))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};

use wind_config::Config;
use wind_engine::EngineManager;

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size() as isize, Relaxed) + l.size() as isize;
            PEAK.fetch_max(now, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size() as isize, Relaxed);
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn mb(b: isize) -> f64 {
    b as f64 / 1024.0 / 1024.0
}

fn status_kb(key: &str) -> i64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn trim() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    unsafe {
        malloc_trim(0);
    }
}

fn data_dir() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../build_dev/data");
    p.join("schemas/pinyin.schema.toml").exists().then_some(p)
}

const PROBE_TEXTS: [&str; 8] = [
    "调研", "中国", "莘莘", "角色", "银行", "长大", "重庆", "觉得",
];

fn child(phase: &str, dir: &std::path::Path) {
    let mut cfg = Config::default();
    cfg.schema.available = vec!["pinyin".to_string()];
    cfg.schema.active = "pinyin".to_string();
    let m = EngineManager::new(&cfg, Some(dir));
    for q in ["nihao", "tiaoyan", "zhongguo", "womenqudiaoyan"] {
        let r = m.convert(q, 9);
        assert!(!r.candidates.is_empty(), "引擎应已载入并出候选：{q}");
    }
    trim();
    let (rss0, live0) = (status_kb("RssAnon:"), LIVE.load(Relaxed));
    PEAK.store(live0, Relaxed);
    let hwm0 = status_kb("VmHWM:");
    let t0 = std::time::Instant::now();
    let mut texts = 0usize;
    let mut found = 0usize;
    if phase != "base" {
        assert!(m.prewarm_reverse_index("pinyin"), "应建成/打开索引");
        let idx = m.reverse_index_if_ready("pinyin").expect("索引就绪");
        texts = idx.len();
        found = PROBE_TEXTS
            .iter()
            .filter(|t| idx.codes_of(t).is_some())
            .count();
    }
    let dt = t0.elapsed();
    let peak = PEAK.load(Relaxed) - live0;
    let hwm = status_kb("VmHWM:");
    trim();
    println!(
        "ROW {phase} {:.0}ms texts={texts} found={found}/{} live+={:.2}MB peak+={:.2}MB \
         RssAnon+={:.2}MB(trimmed) VmHWM {:.1}->{:.1}MB RssAnon_base={:.1}MB",
        dt.as_secs_f64() * 1000.0,
        PROBE_TEXTS.len(),
        mb(LIVE.load(Relaxed) - live0),
        mb(peak),
        (status_kb("RssAnon:") - rss0) as f64 / 1024.0,
        hwm0 as f64 / 1024.0,
        hwm as f64 / 1024.0,
        rss0 as f64 / 1024.0,
    );
}

fn find_wridx(root: &std::path::Path) -> Vec<PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "wridx" || x == "fp") {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(root, &mut v);
    v
}

#[test]
#[ignore = "探针，不参与常规回归"]
fn pinyin_reverse_index_memory() {
    let Some(dir) = data_dir() else {
        eprintln!("跳过：无 build_dev/data");
        return;
    };
    if let Ok(phase) = std::env::var("MR_PROBE_PHASE") {
        child(&phase, &dir);
        return;
    }
    let root = std::env::temp_dir().join(format!("wind_misread_mem_probe_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let exe = std::env::current_exe().unwrap();
    let run = |phase: &str| {
        let out = std::process::Command::new(&exe)
            .args([
                "--ignored",
                "--nocapture",
                "--exact",
                "pinyin_reverse_index_memory",
            ])
            .env("MR_PROBE_PHASE", phase)
            .env("XDG_DATA_HOME", &root)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && text.contains("ROW "),
            "子进程 {phase} 失败（{}）\n{text}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        for l in text.lines().filter(|l| l.starts_with("ROW ")) {
            println!("{}", &l[4..]);
        }
    };
    // 先暖一次 .wdat 等词库缓存，免得基线里混进词库首建。
    run("base");
    for round in 1..=3 {
        println!("-- 轮 {round}");
        run("base");
        for p in find_wridx(&root)
            .iter()
            .filter(|p| p.to_string_lossy().contains("pinyin.wridx"))
        {
            let _ = std::fs::remove_file(p);
        }
        run("build");
        run("reuse");
    }
    let _ = std::fs::remove_dir_all(&root);
}
