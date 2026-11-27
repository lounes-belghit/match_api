use matchs_api::crypto::{generate_api_key, hash_password, sha256_hex, verify_password};
use matchs_api::scraper::{parse_player_stats, MatchDto, PlayersResponse};

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

const LFP_STATS_SAMPLE: &str = r#"
<div id="statistics-tabs-content">
  <div id="players-first-pane">
    <div class="players-stats-upper">
      <div class="top-goal-scorers-cards">
        <a href="/fr/player/40287">
          <div class="stat-player-card d-flex align-items-center">
            <div class="player-rank">1</div>
            <div class="player-club-logo"><img src="/clubs-logos/759-1791978447.png" alt="JSK"></div>
            <div class="player-name flex-grow-1">MOUAKI DADI El hocine</div>
            <div class="player-goals-stat"><span>5</span></div>
          </div>
        </a>
      </div>
    </div>
    <div class="players-stats-lower">
      <div class="stat-player-incident card text-center">
        <h5 class="incident-title">Yellow cards</h5>
        <div class="incident-info">
          <span class="player-name">OJO MOHAMED ASHIRAF</span>
          <span class="incident-count">4</span>
        </div>
      </div>
    </div>
  </div>
  <div id="players-second-pane">
    <div class="statistics-table">
      <table class="table table-borderless">
        <thead><tr><th scope="col">Goal Scorer</th><th scope="col">Club</th><th scope="col">Goals</th><th scope="col">Jersey</th></tr></thead>
        <tbody>
          <tr class="statistics-row">
            <th scope="row"><span class="statistics-index success-index">1</span><span class="statistics-player-name">MOUAKI DADI El hocine</span></th>
            <td>JSK</td><td>5</td><td>9</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</div>"#;

const LIRF_STATS_SAMPLE: &str = r#"
<div class="tab-content" id="statistics-tabs-content">
  <div id="players-first-pane">
    <div class="players-stats-upper">
      <div class="top-participations top-players">
        <div class="top-players-cards">
          <a href="/en/player/254110">
            <div class="stat-player-card d-flex align-items-center">
              <div class="player-rank">1</div>
              <div class="flex-grow-1 player-name-club">
                <div class="player-name">DAHMANI ABBES-BAHAEDDINE</div>
                <div class="player-club-logo"><img src="/clubs-logos/1391-1723229634.png" alt="MCH"><span>MCH</span></div>
              </div>
              <div class="player-stat"><span>12</span></div>
            </div>
          </a>
        </div>
      </div>
    </div>
  </div>
</div>"#;

const PLAYERS_SAMPLE: &str = r#"{
  "club": {"id": 759, "name": "JS Kabylie", "imageUrl": "/clubs-logos/759.png"},
  "seasons": [{}],
  "goalKeepers": [{"id": 10, "name": "Keeper One", "jersey": 1, "age": 27, "position": "Goalkeeper", "imageUrl": "/licence-photo/10.png", "nationality": {"name": "Algeria", "imageUrl": "/flags/dz.png"}}],
  "defenders": [],
  "midfielders": [],
  "forwards": [{"id": 11, "name": "Striker One", "jersey": 9, "age": null, "position": "Forward", "imageUrl": "", "nationality": null}]
}"#;

#[test]
fn parses_lfp_goal_card_incident_and_table() {
    let stats = parse_player_stats(LFP_STATS_SAMPLE).unwrap();
    let goals = stats
        .iter()
        .find(|item| item.stat == "goals" && item.source_player_id == Some(40287))
        .unwrap();
    assert_eq!(goals.name, "MOUAKI DADI El hocine");
    assert_eq!(goals.value, 5);
    assert_eq!(goals.rank, Some(1));
    assert_eq!(
        goals.club_logo_path.as_deref(),
        Some("/clubs-logos/759-1791978447.png")
    );
    assert_eq!(goals.jersey, Some(9));
    assert!(stats
        .iter()
        .any(|item| item.stat == "yellow_cards" && item.value == 4));
    assert_eq!(stats.iter().filter(|item| item.stat == "goals").count(), 1);
}

#[test]
fn parses_lirf_appearance_cards() {
    let stats = parse_player_stats(LIRF_STATS_SAMPLE).unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].stat, "appearances");
    assert_eq!(stats[0].source_player_id, Some(254110));
    assert_eq!(stats[0].value, 12);
    assert_eq!(stats[0].club_name.as_deref(), Some("MCH"));
}

#[test]
fn parses_official_roster_json() {
    let response: PlayersResponse = serde_json::from_str(PLAYERS_SAMPLE).unwrap();
    let players = response.into_players();
    assert_eq!(players.len(), 2);
    assert_eq!(players[0].id_i64(), Some(10));
    assert_eq!(players[0].jersey, Some(1));
    assert_eq!(players[0].nationality.as_ref().unwrap().name, "Algeria");
    assert_eq!(players[1].id_i64(), Some(11));
    assert_eq!(players[1].position, "Forward");
}
