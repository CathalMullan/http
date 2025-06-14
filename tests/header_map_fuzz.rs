#![allow(dead_code, unused_imports)]

use http::header::*;
use http::*;

use quickcheck::{Arbitrary, Gen, QuickCheck, TestResult};
use rand::prelude::IndexedRandom;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use std::collections::HashMap;

#[cfg(not(miri))]
#[test]
fn header_map_fuzz() {
    fn prop(fuzz: Fuzz) -> TestResult {
        fuzz.run();
        TestResult::from_bool(true)
    }

    QuickCheck::new().quickcheck(prop as fn(Fuzz) -> TestResult);
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Fuzz {
    // The magic seed that makes the test case reproducible
    seed: [u8; 32],

    // Actions to perform
    steps: Vec<Step>,

    // Number of steps to drop
    reduce: usize,
}

#[derive(Debug)]
struct Weight {
    insert: usize,
    remove: usize,
    append: usize,
}

#[derive(Debug, Clone)]
struct Step {
    action: Action,
    expect: AltMap,
}

#[derive(Debug, Clone)]
enum Action {
    Insert {
        name: HeaderName,         // Name to insert
        val: HeaderValue,         // Value to insert
        old: Option<HeaderValue>, // Old value
    },
    Append {
        name: HeaderName,
        val: HeaderValue,
        ret: bool,
    },
    Remove {
        name: HeaderName,         // Name to remove
        val: Option<HeaderValue>, // Value to get
    },
}

// An alternate implementation of HeaderMap backed by HashMap
#[derive(Debug, Clone, Default)]
struct AltMap {
    map: HashMap<HeaderName, Vec<HeaderValue>>,
}

impl Fuzz {
    fn new(seed: [u8; 32]) -> Self {
        // Seed the RNG
        let mut rng = StdRng::from_seed(seed);

        let mut steps = vec![];
        let mut expect = AltMap::default();
        let num = rng.random_range(5..500);

        let weight = Weight {
            insert: rng.random_range(1..10),
            remove: rng.random_range(1..10),
            append: rng.random_range(1..10),
        };

        while steps.len() < num {
            steps.push(expect.gen_step(&weight, &mut rng));
        }

        Self {
            seed,
            steps,
            reduce: 0,
        }
    }

    fn run(self) {
        // Create a new header map
        let mut map = HeaderMap::new();

        // Number of steps to perform
        let take = self.steps.len() - self.reduce;

        for step in self.steps.into_iter().take(take) {
            step.action.apply(&mut map);

            step.expect.assert_identical(&map);
        }
    }
}

impl Arbitrary for Fuzz {
    fn arbitrary(_: &mut Gen) -> Self {
        Self::new(rand::rng().random())
    }
}

impl AltMap {
    fn gen_step(&mut self, weight: &Weight, rng: &mut StdRng) -> Step {
        let action = self.gen_action(weight, rng);

        Step {
            action,
            expect: self.clone(),
        }
    }

    /// This will also apply the action against `self`
    fn gen_action(&mut self, weight: &Weight, rng: &mut StdRng) -> Action {
        let sum = weight.insert + weight.remove + weight.append;

        let mut num = rng.random_range(0..sum);

        if num < weight.insert {
            return self.gen_insert(rng);
        }

        num -= weight.insert;

        if num < weight.remove {
            return self.gen_remove(rng);
        }

        num -= weight.remove;

        if num < weight.append {
            return self.gen_append(rng);
        }

        unreachable!();
    }

    fn gen_insert(&mut self, rng: &mut StdRng) -> Action {
        let name = self.gen_name(4, rng);
        let val = gen_header_value(rng);
        let old = self.insert(name.clone(), val.clone());

        Action::Insert { name, val, old }
    }

    fn gen_remove(&mut self, rng: &mut StdRng) -> Action {
        let name = self.gen_name(-4, rng);
        let val = self.remove(&name);

        Action::Remove { name, val }
    }

    fn gen_append(&mut self, rng: &mut StdRng) -> Action {
        let name = self.gen_name(-5, rng);
        let val = gen_header_value(rng);

        let vals = self.map.entry(name.clone()).or_default();

        let ret = !vals.is_empty();
        vals.push(val.clone());

        Action::Append { name, val, ret }
    }

    /// Negative numbers weigh finding an existing header higher
    fn gen_name(&self, weight: i32, rng: &mut StdRng) -> HeaderName {
        let mut existing = rng.random_ratio(1, weight.unsigned_abs());

        if weight < 0 {
            existing = !existing;
        }

        if existing {
            // Existing header
            self.find_random_name(rng)
                .map_or_else(|| gen_header_name(rng), |name| name)
        } else {
            gen_header_name(rng)
        }
    }

    fn find_random_name(&self, rng: &mut StdRng) -> Option<HeaderName> {
        if self.map.is_empty() {
            None
        } else {
            let n = rng.random_range(0..self.map.len());
            self.map.keys().nth(n).cloned()
        }
    }

    fn insert(&mut self, name: HeaderName, val: HeaderValue) -> Option<HeaderValue> {
        let old = self.map.insert(name, vec![val]);
        old.and_then(|v| v.into_iter().next())
    }

    fn remove(&mut self, name: &HeaderName) -> Option<HeaderValue> {
        self.map.remove(name).and_then(|v| v.into_iter().next())
    }

    fn assert_identical(&self, other: &HeaderMap<HeaderValue>) {
        assert_eq!(self.map.len(), other.keys_len());

        for (key, val) in &self.map {
            // Test get
            assert_eq!(other.get(key), val.first());

            // Test get_all
            let vals = other.get_all(key);
            let actual: Vec<_> = vals.iter().collect();
            assert_eq!(&actual[..], &val[..]);
        }
    }
}

impl Action {
    fn apply(self, map: &mut HeaderMap<HeaderValue>) {
        match self {
            Self::Insert { name, val, old } => {
                let actual = map.insert(name, val);
                assert_eq!(actual, old);
            }
            Self::Remove { name, val } => {
                // Just to help track the state, load all associated values.
                drop(map.get_all(&name).iter().collect::<Vec<_>>());

                let actual = map.remove(&name);
                assert_eq!(actual, val);
            }
            Self::Append { name, val, ret } => {
                assert_eq!(ret, map.append(name, val));
            }
        }
    }
}

fn gen_header_name(g: &mut StdRng) -> HeaderName {
    const STANDARD_HEADERS: &[HeaderName] = &[
        header::ACCEPT,
        header::ACCEPT_CHARSET,
        header::ACCEPT_ENCODING,
        header::ACCEPT_LANGUAGE,
        header::ACCEPT_RANGES,
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        header::ACCESS_CONTROL_MAX_AGE,
        header::ACCESS_CONTROL_REQUEST_HEADERS,
        header::ACCESS_CONTROL_REQUEST_METHOD,
        header::AGE,
        header::ALLOW,
        header::ALT_SVC,
        header::AUTHORIZATION,
        header::CACHE_CONTROL,
        header::CACHE_STATUS,
        header::CDN_CACHE_CONTROL,
        header::CONNECTION,
        header::CONTENT_DISPOSITION,
        header::CONTENT_ENCODING,
        header::CONTENT_LANGUAGE,
        header::CONTENT_LENGTH,
        header::CONTENT_LOCATION,
        header::CONTENT_RANGE,
        header::CONTENT_SECURITY_POLICY,
        header::CONTENT_SECURITY_POLICY_REPORT_ONLY,
        header::CONTENT_TYPE,
        header::COOKIE,
        header::DNT,
        header::DATE,
        header::ETAG,
        header::EXPECT,
        header::EXPIRES,
        header::FORWARDED,
        header::FROM,
        header::HOST,
        header::IF_MATCH,
        header::IF_MODIFIED_SINCE,
        header::IF_NONE_MATCH,
        header::IF_RANGE,
        header::IF_UNMODIFIED_SINCE,
        header::LAST_MODIFIED,
        header::LINK,
        header::LOCATION,
        header::MAX_FORWARDS,
        header::ORIGIN,
        header::PRAGMA,
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
        header::PUBLIC_KEY_PINS,
        header::PUBLIC_KEY_PINS_REPORT_ONLY,
        header::RANGE,
        header::REFERER,
        header::REFERRER_POLICY,
        header::REFRESH,
        header::RETRY_AFTER,
        header::SEC_WEBSOCKET_ACCEPT,
        header::SEC_WEBSOCKET_EXTENSIONS,
        header::SEC_WEBSOCKET_KEY,
        header::SEC_WEBSOCKET_PROTOCOL,
        header::SEC_WEBSOCKET_VERSION,
        header::SERVER,
        header::SET_COOKIE,
        header::STRICT_TRANSPORT_SECURITY,
        header::TE,
        header::TRAILER,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
        header::UPGRADE_INSECURE_REQUESTS,
        header::USER_AGENT,
        header::VARY,
        header::VIA,
        header::WARNING,
        header::WWW_AUTHENTICATE,
        header::X_CONTENT_TYPE_OPTIONS,
        header::X_DNS_PREFETCH_CONTROL,
        header::X_FRAME_OPTIONS,
        header::X_XSS_PROTECTION,
    ];

    if g.random_ratio(1, 2) {
        STANDARD_HEADERS.choose(g).unwrap().clone()
    } else {
        let value = gen_string(g, 1, 25);
        HeaderName::from_bytes(value.as_bytes()).unwrap()
    }
}

fn gen_header_value(g: &mut StdRng) -> HeaderValue {
    let value = gen_string(g, 0, 70);
    HeaderValue::from_bytes(value.as_bytes()).unwrap()
}

fn gen_string(g: &mut StdRng, min: usize, max: usize) -> String {
    let bytes: Vec<_> = (min..max)
        .map(|_| {
            // Chars to pick from
            *b"ABCDEFGHIJKLMNOPQRSTUVabcdefghilpqrstuvwxyz----"
                .choose(g)
                .unwrap()
        })
        .collect();

    String::from_utf8(bytes).unwrap()
}
