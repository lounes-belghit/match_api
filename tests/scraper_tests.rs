use matchs_api::crypto::{generate_api_key, hash_password, sha256_hex, verify_password};
use matchs_api::scraper::MatchDto;

const LFP_SAMPLE: &str = r#"{
  "id": 222327,
  "homeClub": {"id": 1, "name": "JS Kabylie", "surname": "JSK", "imageUrl": "/clubs-logos/1.png"},
  "awayClub": {"id": 2, "name": "MC Alger", "surname": "MCA", "imageUrl": "/clubs-logos/2.png"},
  "group": {"id": 4251, "name": "Ligue 1"},
  "homeScore": "1", "awayScore": "1",
  "date": "lundi 23 novembre 2026", "time": "17:00", "stade": "Stade du 1er Novembre",
  "images": [], "videos": [], "live": false, "category": "Ligue 1", "noAudience": false, "round": "11"
}"#;

const LIRF_SAMPLE: &str = r#"{
  "id": 226932,
  "homeClub": {"id": 10, "name": "US Remchi", "surname": "USR", "imageUrl": ""},
  "awayClub": {"id": 11, "name": "IS Tighenif", "surname": "IST", "imageUrl": ""},
  "group": {"id": 4277, "name": "OUEST"},
  "homeScore": "-", "awayScore": "-",
  "date": "Tuesday 1 December 2026", "time": "15:00", "stade": "Stade Municipal",
  "images": [], "videos": [], "live": false, "liveTime": "", "matchStartTimestamp": 1796133600,
  "decision": "NOT_PLAYED", "stopped": false, "forfieted": false,
  "generalWhithDrawAl": false, "penalized": false, "lost": false,
  "round": "8", "category": "Inter-régions", "noAudience": false, "url": "/match/226932"
}"#;

#[test]
fn parses_lfp_french_date_and_score() {
    let m: MatchDto = serde_json::from_str(LFP_SAMPLE).unwrap();
    assert_eq!(m.id_i64(), Some(222327));
    assert_eq!(m.round_number(), 11);
    assert_eq!(m.status(), "finished");
    assert_eq!(m.scores(), (Some(1), Some(1)));
    let kickoff = m.kickoff_utc().unwrap();
    assert_eq!(kickoff.to_rfc3339(), "2026-11-23T16:00:00+00:00");
}

#[test]
fn parses_lirf_timestamp_and_scheduled_status() {
    let m: MatchDto = serde_json::from_str(LIRF_SAMPLE).unwrap();
    assert_eq!(m.status(), "scheduled");
    assert_eq!(m.scores(), (None, None));
    assert_eq!(m.source_path(), "/match/226932");
    assert!(m.kickoff_utc().is_some());
}

#[test]
fn password_hash_roundtrip() {
    let hash = hash_password("correct horse battery staple").unwrap();
    assert!(verify_password("correct horse battery staple", &hash));
    assert!(!verify_password("wrong password", &hash));
}

#[test]
fn api_key_has_expected_shape() {
    let key = generate_api_key();
    assert!(key.raw.starts_with(&format!("m3_{}_", key.prefix)));
    assert_eq!(key.hash, sha256_hex(&key.raw));
}
