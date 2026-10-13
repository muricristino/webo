use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const MARK: &str = "pm2:";
const CLK_TCK: u64 = 100;
const PAGE_SIZE: u64 = 4096;
const FIRST_READ_BYTES: u64 = 1 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub name: String,
    pub interpreter: String,
    pub script: String,
    pub pid_file: String,
    pub out_log: String,
    pub err_log: String,
    pub restarts: i64,
}

pub fn home() -> Option<PathBuf> {
    std::env::var("WEBO_PM2_HOME")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn compose_key(name: &str) -> String {
    format!("{MARK}{name}")
}

pub fn is_pm2(compose_project: Option<&str>) -> bool {
    compose_project.is_some_and(|c| c.starts_with(MARK))
}

pub fn slug(name: &str) -> String {
    format!("pm2-{}", crate::projects::slug_for(name))
}

fn base(path: &str) -> String {
    Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
}

// dump.pm2 also carries each app's full environment, secrets included: only
// the fields below are ever read.
pub fn parse_dump(text: &str) -> Vec<App> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|a| {
            let s = |k: &str| a.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let name = s("name");
            if name.is_empty() {
                return None;
            }
            Some(App {
                interpreter: s("exec_interpreter"),
                script: s("pm_exec_path"),
                pid_file: base(&s("pm_pid_path")),
                out_log: base(&s("pm_out_log_path")),
                err_log: base(&s("pm_err_log_path")),
                restarts: a.get("restart_time").and_then(|v| v.as_i64()).unwrap_or(0),
                name,
            })
        })
        .collect()
}

pub fn read_apps(home: &Path) -> Vec<App> {
    fs::read_to_string(home.join("dump.pm2")).map(|t| parse_dump(&t)).unwrap_or_default()
}

pub fn tech_of(app: &App) -> &'static str {
    let bin = base(&app.interpreter);
    let script = base(&app.script);
    if bin == "node" || bin.starts_with("node") || script == "npm" || script.ends_with(".js") {
        "node"
    } else if bin.starts_with("python") || script.ends_with(".py") {
        "python"
    } else if bin == "bun" {
        "bun"
    } else if bin == "bash" || bin == "sh" || script.ends_with(".sh") {
        "shell"
    } else {
        "generic"
    }
}

pub fn pid_of(home: &Path, proc_root: &Path, app: &App) -> Option<u32> {
    if app.pid_file.is_empty() {
        return None;
    }
    let pid: u32 = fs::read_to_string(home.join("pids").join(&app.pid_file)).ok()?.trim().parse().ok()?;
    proc_root.join(pid.to_string()).join("stat").exists().then_some(pid)
}

pub fn tree(root: u32, parents: &[(u32, u32)]) -> Vec<u32> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let p = out[i];
        out.extend(parents.iter().filter(|(_, pp)| *pp == p).map(|(pid, _)| *pid));
        i += 1;
    }
    out
}

pub struct Stat {
    pub ppid: u32,
    pub ticks: u64,
    pub start: u64,
}

pub fn parse_stat(stat: &str) -> Option<Stat> {
    let close = stat.rfind(')')?;
    let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    if rest.len() < 20 {
        return None;
    }
    let n = |i: usize| rest[i].parse::<u64>().unwrap_or(0);
    Some(Stat { ppid: n(1) as u32, ticks: n(11) + n(12), start: n(19) })
}

pub fn parse_rss(statm: &str) -> u64 {
    statm.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * PAGE_SIZE
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub ticks: u64,
    pub rss: u64,
    pub uptime_secs: u64,
}

pub fn usage(proc_root: &Path, pid: u32) -> Usage {
    let mut stats: HashMap<u32, Stat> = HashMap::new();
    if let Ok(entries) = fs::read_dir(proc_root) {
        for e in entries.flatten() {
            let Ok(p) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
            if let Some(s) = fs::read_to_string(e.path().join("stat")).ok().and_then(|t| parse_stat(&t)) {
                stats.insert(p, s);
            }
        }
    }
    let parents: Vec<(u32, u32)> = stats.iter().map(|(p, s)| (*p, s.ppid)).collect();
    let mut u = Usage::default();
    for p in tree(pid, &parents) {
        if let Some(s) = stats.get(&p) {
            u.ticks += s.ticks;
        }
        u.rss += fs::read_to_string(proc_root.join(p.to_string()).join("statm")).map(|t| parse_rss(&t)).unwrap_or(0);
    }
    let host_up = fs::read_to_string(proc_root.join("uptime"))
        .ok()
        .and_then(|t| t.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
        .unwrap_or(0.0);
    if let Some(s) = stats.get(&pid) {
        u.uptime_secs = (host_up - (s.start / CLK_TCK) as f64).max(0.0) as u64;
    }
    u
}

pub fn collect(
    home: &Path,
    proc_root: &Path,
    prev: &mut HashMap<String, u64>,
    secs: u64,
) -> Vec<(App, Option<crate::metrics::ProjectLive>)> {
    let apps = read_apps(home);
    prev.retain(|name, _| apps.iter().any(|a| &a.name == name));
    apps.into_iter()
        .map(|app| {
            let Some(pid) = pid_of(home, proc_root, &app) else {
                prev.remove(&app.name);
                return (app, None);
            };
            let u = usage(proc_root, pid);
            let cpu = prev.get(&app.name).map(|t| cpu_pct(u.ticks.saturating_sub(*t), secs)).unwrap_or(0.0);
            prev.insert(app.name.clone(), u.ticks);
            let live = crate::metrics::ProjectLive {
                containers: vec![crate::metrics::ProjectContainer {
                    name: app.name.clone(),
                    role: "app".into(),
                    image: app.script.clone(),
                    state: "running".into(),
                    restarts: app.restarts,
                    uptime_secs: u.uptime_secs,
                    cpu_pct: cpu,
                    mem_bytes: u.rss,
                    disk_bps: 0,
                }],
                cpu_pct: cpu,
                mem_bytes: u.rss,
                ..Default::default()
            };
            (app, Some(live))
        })
        .collect()
}

pub fn cpu_pct(ticks_delta: u64, secs: u64) -> f32 {
    (ticks_delta as f64 / (secs.max(1) * CLK_TCK) as f64 * 100.0) as f32
}

// `pm2 start --time` prefixes each line with "YYYY-MM-DDTHH:MM:SS: "; without
// it the line carries no time and gets the moment it was read.
pub fn parse_line(raw: &str, now: i64) -> (i64, String) {
    if raw.len() > 21 && raw.as_bytes()[19] == b':' && raw.as_bytes()[20] == b' ' {
        let stamp = format!("{}Z", &raw[..19]);
        if let Ok(t) = time::OffsetDateTime::parse(&stamp, &time::format_description::well_known::Rfc3339) {
            return (t.unix_timestamp(), raw[21..].to_string());
        }
    }
    (now, raw.to_string())
}

pub fn complete_lines(buf: &str) -> (Vec<&str>, usize) {
    let Some(end) = buf.rfind('\n') else { return (Vec::new(), 0) };
    (buf[..end].split('\n').filter(|l| !l.trim().is_empty()).collect(), end + 1)
}

pub fn start_offset(len: u64, already_indexed: bool) -> u64 {
    if already_indexed {
        len
    } else {
        len.saturating_sub(FIRST_READ_BYTES)
    }
}

pub fn read_from(path: &Path, offset: u64) -> Option<(String, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let from = if offset > len { 0 } else { offset };
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).ok()?;
    Some((String::from_utf8_lossy(&bytes).into_owned(), from))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = r#"[
      {"name":"api","exec_interpreter":"node","pm_exec_path":"/opt/api/server.js",
       "pm_pid_path":"/root/.pm2/pids/api-0.pid","pm_out_log_path":"/root/.pm2/logs/api-out.log",
       "pm_err_log_path":"/root/.pm2/logs/api-error.log","restart_time":3,
       "env":{"SECRET":"s3cr3t"},"PAGARME_SECRET":"nope"},
      {"name":"jobs","exec_interpreter":"bash","pm_exec_path":"/opt/jobs/run.sh",
       "pm_pid_path":"/root/.pm2/pids/jobs-1.pid","pm_out_log_path":"/root/.pm2/logs/jobs-out.log",
       "pm_err_log_path":"/root/.pm2/logs/jobs-error.log"},
      {"exec_interpreter":"node"}
    ]"#;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("webo-pm2-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_dump_gives_the_apps_and_never_their_environment() {
        let apps = parse_dump(DUMP);
        assert_eq!(apps.len(), 2, "an entry with no name is skipped");
        assert_eq!(apps[0].name, "api");
        assert_eq!(apps[0].pid_file, "api-0.pid");
        assert_eq!(apps[0].out_log, "api-out.log");
        assert_eq!(apps[0].restarts, 3);
        assert!(!format!("{apps:?}").contains("s3cr3t"));
        assert!(!format!("{apps:?}").contains("nope"));
        assert!(parse_dump("not json").is_empty());
        assert!(parse_dump("{}").is_empty());
    }

    #[test]
    fn tech_comes_from_the_interpreter_or_the_script() {
        let mut a = parse_dump(DUMP)[0].clone();
        assert_eq!(tech_of(&a), "node");
        a.interpreter = "/usr/bin/node".into();
        a.script = "/usr/bin/npm".into();
        assert_eq!(tech_of(&a), "node");
        a.interpreter = "bash".into();
        a.script = "/opt/x/run.sh".into();
        assert_eq!(tech_of(&a), "shell");
        a.interpreter = "python3".into();
        assert_eq!(tech_of(&a), "python");
        a.interpreter = "bun".into();
        a.script = "/x/index.ts".into();
        assert_eq!(tech_of(&a), "bun");
        a.interpreter = "ruby".into();
        assert_eq!(tech_of(&a), "generic");
    }

    #[test]
    fn names_and_marks() {
        assert_eq!(compose_key("radar-jobs"), "pm2:radar-jobs");
        assert!(is_pm2(Some("pm2:radar-jobs")));
        assert!(!is_pm2(Some("codo")));
        assert!(!is_pm2(None));
        assert_eq!(slug("Radar Jobs"), "pm2-radar-jobs");
    }

    #[test]
    fn a_pid_counts_only_while_its_process_lives() {
        let home = scratch("pid");
        let proc_root = scratch("pidproc");
        fs::create_dir_all(home.join("pids")).unwrap();
        fs::write(home.join("pids/api-0.pid"), "4242\n").unwrap();
        let app = parse_dump(DUMP)[0].clone();
        assert_eq!(pid_of(&home, &proc_root, &app), None, "dead process");
        fs::create_dir_all(proc_root.join("4242")).unwrap();
        fs::write(proc_root.join("4242/stat"), "x").unwrap();
        assert_eq!(pid_of(&home, &proc_root, &app), Some(4242));
        let mut none = app.clone();
        none.pid_file = String::new();
        assert_eq!(pid_of(&home, &proc_root, &none), None);
    }

    #[test]
    fn usage_sums_the_whole_process_tree() {
        let proc_root = scratch("usage");
        let stat = |pid: u32, ppid: u32, ut: u64, st: u64, start: u64| {
            format!("{pid} (n o) S {ppid} 0 0 0 0 0 0 0 0 0 {ut} {st} 0 0 0 0 1 0 {start} 0")
        };
        for (pid, ppid, ut, st) in [(10, 1, 50, 10), (11, 10, 20, 0), (12, 11, 5, 5), (99, 1, 1000, 0)] {
            fs::create_dir_all(proc_root.join(pid.to_string())).unwrap();
            fs::write(proc_root.join(format!("{pid}/stat")), stat(pid, ppid, ut, st, 1000)).unwrap();
            fs::write(proc_root.join(format!("{pid}/statm")), "100 25 0").unwrap();
        }
        fs::write(proc_root.join("uptime"), "100.5 400.0").unwrap();
        let u = usage(&proc_root, 10);
        assert_eq!(u.ticks, 90, "parent and descendants, not the unrelated 99");
        assert_eq!(u.rss, 3 * 25 * PAGE_SIZE);
        assert_eq!(u.uptime_secs, 90);
        assert_eq!(usage(&scratch("empty"), 1), Usage::default());
    }

    #[test]
    fn collect_reports_running_apps_and_lists_stopped_ones_without_live_data() {
        let home = scratch("collect");
        let proc_root = scratch("collectproc");
        fs::write(home.join("dump.pm2"), DUMP).unwrap();
        fs::create_dir_all(home.join("pids")).unwrap();
        fs::write(home.join("pids/api-0.pid"), "7").unwrap();
        fs::write(home.join("pids/jobs-1.pid"), "8").unwrap();
        fs::create_dir_all(proc_root.join("7")).unwrap();
        let write_stat = |ticks: u64| {
            fs::write(proc_root.join("7/stat"), format!("7 (node) S 1 0 0 0 0 0 0 0 0 0 {ticks} 0 0 0 0 0 1 0 100 0")).unwrap();
        };
        write_stat(100);
        fs::write(proc_root.join("7/statm"), "10 4 0").unwrap();
        let mut prev = HashMap::from([("gone".to_string(), 1u64)]);

        let first = collect(&home, &proc_root, &mut prev, 10);
        assert_eq!(first.len(), 2);
        let api = first[0].1.as_ref().expect("api is running");
        assert_eq!(api.containers[0].name, "api");
        assert_eq!(api.containers[0].restarts, 3);
        assert_eq!(api.mem_bytes, 4 * PAGE_SIZE);
        assert_eq!(api.cpu_pct, 0.0, "no previous sample yet");
        assert!(first[1].1.is_none(), "jobs has a pid file but no live process");
        assert!(!prev.contains_key("gone"), "apps no longer in the dump are forgotten");

        write_stat(150);
        let second = collect(&home, &proc_root, &mut prev, 10);
        assert!((second[0].1.as_ref().unwrap().cpu_pct - 5.0).abs() < 0.001);
    }

    #[test]
    fn stat_and_statm_parsing() {
        assert!(parse_stat("1 (x) S 0").is_none());
        assert!(parse_stat("no parens").is_none());
        assert_eq!(parse_rss("garbage"), 0);
        assert_eq!(tree(1, &[(2, 1), (3, 2), (4, 9)]), vec![1, 2, 3]);
        assert!((cpu_pct(150, 15) - 10.0).abs() < 0.001);
        assert!((cpu_pct(100, 0) - 100.0).abs() < 0.001);
    }

    #[test]
    fn lines_carry_their_own_time_when_pm2_wrote_one() {
        let (ts, text) = parse_line("2026-10-13T20:30:01: GET /health 200", 5);
        assert_eq!(ts, 1_791_923_401);
        assert_eq!(text, "GET /health 200");
        assert_eq!(parse_line("plain line", 5), (5, "plain line".into()));
        assert_eq!(parse_line("2026-13-45T99:99:99: bad", 5).0, 5);
    }

    #[test]
    fn only_complete_lines_are_taken() {
        assert_eq!(complete_lines("a\nb\npart"), (vec!["a", "b"], 4));
        assert_eq!(complete_lines("a\n\n  \nb\n"), (vec!["a", "b"], 8));
        assert_eq!(complete_lines("no newline yet"), (Vec::<&str>::new(), 0));
    }

    #[test]
    fn the_first_read_is_bounded_and_a_restart_does_not_reindex() {
        assert_eq!(start_offset(100, false), 0);
        assert_eq!(start_offset(5 << 20, false), 4 << 20);
        assert_eq!(start_offset(5 << 20, true), 5 << 20);
    }

    #[test]
    fn reading_resumes_and_survives_truncation() {
        let d = scratch("read");
        let f = d.join("out.log");
        fs::write(&f, "one\ntwo\n").unwrap();
        assert_eq!(read_from(&f, 4), Some(("two\n".into(), 4)));
        fs::write(&f, "x\n").unwrap();
        assert_eq!(read_from(&f, 8), Some(("x\n".into(), 0)), "rotated file starts over");
        assert_eq!(read_from(&d.join("missing"), 0), None);
    }

    #[test]
    fn home_comes_from_the_environment() {
        let _env = crate::testutil::env_lock();
        std::env::remove_var("WEBO_PM2_HOME");
        assert_eq!(home(), None);
        std::env::set_var("WEBO_PM2_HOME", " /host/pm2 ");
        assert_eq!(home(), Some(PathBuf::from("/host/pm2")));
        std::env::set_var("WEBO_PM2_HOME", "  ");
        assert_eq!(home(), None);
        std::env::remove_var("WEBO_PM2_HOME");
        let h = scratch("apps");
        fs::write(h.join("dump.pm2"), DUMP).unwrap();
        assert_eq!(read_apps(&h).len(), 2);
        assert!(read_apps(&scratch("noapps")).is_empty());
    }
}
