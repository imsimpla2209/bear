use std::env;

use actix_web::{cookie};
use actix_web::cookie::SameSite;
use std::io::{Write};
use log::{Level, LevelFilter};
use metrics::{gauge};
use rand::{Rng, RngCore};

// everything here candidate for reuse

/** Generates a secure token */
pub fn gentoken<T: From<String>>() -> T {
    let mut buf = [0; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    T::from(bs58::encode(buf).into_string())
}

// for now capital letters and numbers
pub fn gen_alphabetical(len: usize) -> String {
    let mut result = rand::thread_rng().sample_iter(rand::distributions::Alphanumeric).take(len).map(char::from).collect::<String>();
    result.make_ascii_uppercase();
    result
}

#[allow(unused)]
pub fn debug<T: std::fmt::Debug>(name: &str, v: T) -> T {
    log::info!("> {}={:?}", name, v);
    v
}

pub fn logging_init(in_test: bool) {
    let with_journal = env::var("WITH_JOURNAL").unwrap_or(String::from("")) == "1";
    env_logger::builder()
        .is_test(in_test)
        .filter_level(LevelFilter::Debug)
        .format(move |buf, record| {
            if with_journal {
                writeln!(buf, "{} {:?} {}",
                         record.level(),
                         std::thread::current().id(),
                         record.args()
                )

            } else {
                let white = format!("[{} {} {:?} {:?}:{}] {}",
                                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                                    record.level(),
                                    std::thread::current().id(),
                                    //record.file().unwrap_or(""),
                                    record.target(),
                                    record.line().unwrap_or(0),
                                    record.args());
                let color = match record.level() {
                    Level::Error => ansi_term::Colour::Red,
                    Level::Debug => ansi_term::Colour::Green,
                    Level::Warn => ansi_term::Colour::Yellow,
                    _ => ansi_term::Colour::White,
                };
                writeln!(buf, "{}", color.paint(white))
            }
        })
        .parse_default_env()
        .init();
}


pub fn std_cookie<'a>(name: &'a str, code: &'a str) -> cookie::Cookie<'a> {
    cookie::Cookie::build(name, code)
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax) // good enough as long as we don't have actionable GET admin requests
        .path("/")
        .finish()
}


// .:-------.:.------:. .:------.:.-------:.
pub type Instant = i64;

pub trait Clock : Send + Sync {
    // returns seconds
    fn utcnow(self: &Self) -> Instant;
    fn advance(self: &mut Self, by: i64) -> Instant;
}

pub struct RealClock();

impl RealClock {
    pub fn new() -> Self {
        Self {}
    }
}

impl Clock for RealClock {
    // seconds timestamp
    fn utcnow(self: &Self) -> Instant { chrono::offset::Utc::now().timestamp() }
    fn advance(self: &mut Self, _by: i64) -> Instant {
        panic!("Using real clock, can't manually advance.")
    }
}

pub struct MockClock {
    pub time: Instant
}

impl MockClock {
    #![allow(dead_code)]
    pub fn new() -> MockClock {
        MockClock { time: 1000 }
    }

}

impl Clone for MockClock {
    fn clone(&self) -> Self {
        MockClock { time: self.time }
    }
}

impl Clock for MockClock {
    fn utcnow(self: &Self) -> Instant { self.time }

    fn advance(self: &mut Self, by: i64) -> Instant {
        self.time += by;
        self.time
    }
}

pub struct TimeMetric {
    pub started: std::time::Instant,
    pub label: &'static str,

}

impl TimeMetric {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            started: std::time::Instant::now(),
        }
    }
}

impl Drop for TimeMetric {
    fn drop(&mut self) {
        gauge!(self.label, self.started.elapsed().as_secs_f64());
    }
}
