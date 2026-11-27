use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, QueryBuilder};
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::crypto::sha256_hex;
use crate::models::{
    ApiError, ApiResult, Club, Competition, FootballGroup, FormOutcome, HeadToHead, Match,
    MatchEvent, Page, Season, Standing, TeamForm,
};
use crate::AppState;

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/", get(landing))
        .route("/docs", get(docs))
        .route("/openapi.json", get(openapi))
        .nest(
            "/api/v1",
            Router::new()
                .route("/competitions", get(competitions))
                .route("/seasons", get(seasons))
                .route("/groups", get(groups))
                .route("/clubs", get(clubs))
                .route("/matches", get(matches))
                .route("/matches/{id}", get(match_by_id))
                .route("/matches/{id}/events", get(match_events))
                .route("/teams/{id}/form", get(team_form))
                .route("/teams/{id}/head-to-head", get(head_to_head))
                .route("/live", get(live_matches))
                .route("/standings", get(standings))
                .route("/search", get(search))
                .route("/status", get(status))
                .layer(middleware::from_fn_with_state(state, auth_headers)),
        )
        .layer(middleware::from_fn(security_headers))
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn ready(State(app): State<AppState>) -> ApiResult<&'static str> {
    app.ready().await?;
    Ok("ok")
}

pub async fn auth_headers(
    State(app): State<AppState>,
    request: Request,
    next: Next,
) -> ApiResult<Response> {
    let (mut parts, body) = request.into_parts();
    let auth = AuthContext::authenticate(&mut parts, &app).await?;
    let headers = auth.response_headers();
    parts.extensions.insert(auth);
    let mut response = next.run(Request::from_parts(parts, body)).await;
    let response_headers = response.headers_mut();
    for (name, value) in headers {
        response_headers.insert(name, value);
    }
    Ok(response)
}

async fn security_headers(request: Request, next: Next) -> Response {
    let request_id = request_id(&request);
    let path = request.uri().path().to_string();
    let is_api_get = request.method() == Method::GET && path.starts_with("/api/v1/");
    let if_none_match = request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut response = next.run(request).await;
    {
        let headers = response.headers_mut();
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
        headers.insert(
            "Permissions-Policy",
            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        );
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains; preload"),
        );
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; img-src 'self' data: https:; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
            ),
        );
        headers.insert(
            HeaderName::from_static("x-request-id"),
            HeaderValue::from_str(&request_id)
                .unwrap_or_else(|_| HeaderValue::from_static("unknown")),
        );
    }
    if is_api_get && response.status() == StatusCode::OK && path == "/api/v1/status" {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(cache_control(&path)),
        );
    }
    if is_api_get && response.status() == StatusCode::OK && path != "/api/v1/status" {
        let (mut parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, 16 * 1024 * 1024)
            .await
            .unwrap_or_default();
        let etag = format!("\"sha256-{}\"", sha256_hex(&bytes));
        parts.headers.insert(
            header::ETAG,
            HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"\"")),
        );
        parts.headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(cache_control(&path)),
        );
        let not_modified = if_none_match
            .as_deref()
            .map(|value| value.split(',').any(|candidate| candidate.trim() == etag))
            .unwrap_or(false);
        if not_modified {
            parts.status = StatusCode::NOT_MODIFIED;
            parts.headers.remove(header::CONTENT_LENGTH);
            parts.headers.remove(header::CONTENT_TYPE);
            response = Response::from_parts(parts, Body::empty());
        } else {
            response = Response::from_parts(parts, Body::from(bytes));
        }
    }
    response
}

fn request_id(request: &Request) -> String {
    request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn cache_control(path: &str) -> &'static str {
    match path {
        "/api/v1/status" => "no-store",
        "/api/v1/live" => "private, max-age=15",
        _ if path.starts_with("/api/v1/") => "private, max-age=60",
        _ => "no-store",
    }
}

async fn landing() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Matchs API — Algerian football data</title>
<meta name="description" content="Fixtures, results, standings and near-live status for Algerian Ligue 1, Ligue 2 and Inter-régions.">
<style>
:root{--ink:#0f172a;--muted:#64748b;--teal:#0f766e;--teal2:#14b8a6;--bg:#f8fafc;--line:#e2e8f0}
*{box-sizing:border-box}
body{margin:0;font-family:Inter,ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif;color:var(--ink);background:radial-gradient(circle at 15% 0%,#ccfbf1 0,transparent 32%),radial-gradient(circle at 85% 10%,#dbeafe 0,transparent 30%),var(--bg);line-height:1.6}
a{color:inherit;text-decoration:none}
.wrap{max-width:1120px;margin:auto;padding:0 24px}
header{display:flex;align-items:center;justify-content:space-between;padding:22px 0}
.brand{display:flex;align-items:center;gap:10px;font-weight:800;letter-spacing:-.02em}
.mark{width:34px;height:34px;border-radius:10px;background:linear-gradient(135deg,var(--teal),var(--teal2));display:grid;place-items:center;color:white;font-size:18px;font-weight:900}
nav{display:flex;gap:22px;align-items:center;font-size:14px;color:var(--muted)}
nav a:hover{color:var(--teal)}
.btn{display:inline-flex;align-items:center;justify-content:center;gap:8px;padding:12px 18px;border-radius:10px;font-weight:700;font-size:14px;transition:.15s ease}
.btn-primary{background:var(--teal);color:white;box-shadow:0 8px 24px #0f766e33}
.btn-primary:hover{background:#115e59;transform:translateY(-1px)}
.btn-ghost{border:1px solid var(--line);background:white;color:var(--ink)}
.btn-ghost:hover{border-color:var(--teal);color:var(--teal)}
.hero{padding:64px 0 44px;display:grid;grid-template-columns:1.15fr .85fr;gap:48px;align-items:center}
.pill{display:inline-flex;align-items:center;gap:8px;padding:6px 12px;border-radius:999px;background:#ecfdf5;color:#047857;font-size:12px;font-weight:800;letter-spacing:.04em;text-transform:uppercase}
.dot{width:7px;height:7px;border-radius:50%;background:#10b981;box-shadow:0 0 0 4px #10b98122}
h1{font-size:clamp(38px,5vw,64px);line-height:1.05;letter-spacing:-.04em;margin:20px 0 18px}
h1 span{color:var(--teal)}
.lead{font-size:19px;color:var(--muted);max-width:620px}
.actions{display:flex;gap:12px;flex-wrap:wrap;margin-top:28px}
.note{margin-top:14px;font-size:13px;color:var(--muted)}
.window{background:#0f172a;border-radius:18px;padding:10px;box-shadow:0 24px 60px #0f172a33;transform:rotate(1deg)}
.bar{display:flex;gap:6px;padding:8px 10px}
.bar i{width:10px;height:10px;border-radius:50%;background:#475569;display:block}
.bar i:nth-child(1){background:#f87171}.bar i:nth-child(2){background:#fbbf24}.bar i:nth-child(3){background:#34d399}
pre{margin:0;background:#020617;color:#e2e8f0;border-radius:12px;padding:20px;overflow:auto;font-size:13px;line-height:1.7}
pre .k{color:#5eead4}pre .s{color:#fde68a}pre .c{color:#64748b}
.stats{display:grid;grid-template-columns:repeat(3,1fr);gap:16px;margin:10px 0 60px}
.stat{background:white;border:1px solid var(--line);border-radius:14px;padding:18px;text-align:center}
.stat b{display:block;font-size:26px;letter-spacing:-.03em}
.stat span{font-size:13px;color:var(--muted)}
section{padding:20px 0 60px}
.section-head{max-width:680px;margin-bottom:26px}
h2{font-size:32px;letter-spacing:-.03em;margin:0 0 10px}
.section-head p{color:var(--muted);margin:0}
.grid{display:grid;grid-template-columns:repeat(3,1fr);gap:18px}
.card{background:white;border:1px solid var(--line);border-radius:16px;padding:22px;box-shadow:0 4px 16px #0f172a08}
.card h3{margin:0 0 8px;font-size:17px}
.card p{margin:0;color:var(--muted);font-size:14px}
.tag{display:inline-block;margin-bottom:12px;padding:4px 9px;border-radius:7px;background:#f1f5f9;color:#475569;font-size:11px;font-weight:800;letter-spacing:.05em}
.steps{display:grid;grid-template-columns:repeat(3,1fr);gap:18px;counter-reset:step}
.step{background:white;border:1px solid var(--line);border-radius:16px;padding:22px;position:relative}
.step:before{counter-increment:step;content:counter(step);position:absolute;top:-14px;left:22px;width:28px;height:28px;border-radius:9px;background:var(--teal);color:white;display:grid;place-items:center;font-weight:800;font-size:13px}
.step h3{margin:12px 0 6px}
.step p{margin:0;color:var(--muted);font-size:14px}
.cta{background:linear-gradient(135deg,#0f766e,#0f172a);color:white;border-radius:20px;padding:38px;display:flex;align-items:center;justify-content:space-between;gap:24px;flex-wrap:wrap}
.cta h2{margin:0 0 6px}
.cta p{margin:0;color:#ccfbf1}
.cta .btn-ghost{background:#ffffff12;border-color:#ffffff33;color:white}
footer{padding:28px 0 44px;border-top:1px solid var(--line);color:var(--muted);font-size:13px;display:flex;justify-content:space-between;gap:16px;flex-wrap:wrap}
@media(max-width:860px){.hero{grid-template-columns:1fr;padding-top:40px}.window{transform:none}.grid,.steps{grid-template-columns:1fr}.stats{grid-template-columns:1fr}.cta{padding:26px}nav{gap:14px}}
</style>
</head>
<body>
<div class="wrap">
<header>
  <a class="brand" href="/"><span class="mark">M</span> Matchs API</a>
  <nav><a href="/docs">Docs</a><a href="/login">Log in</a><a class="btn btn-primary" href="/request-access">Request API key</a></nav>
</header>
<main>
  <div class="hero">
    <div>
      <span class="pill"><span class="dot"></span> v1 is live</span>
      <h1>Algerian football data,<br><span>ready for your app.</span></h1>
      <p class="lead">Fixtures, results, standings and near-live status for Ligue 1, Ligue 2 and Inter-régions (D3) — served from one clean, documented API.</p>
      <div class="actions">
        <a class="btn btn-primary" href="/request-access">Request an API key</a>
        <a class="btn btn-ghost" href="/docs">Read the docs</a>
      </div>
      <p class="note">Create an account, generate a key in the dashboard, and start calling <code>/api/v1/*</code>.</p>
    </div>
    <div class="window" aria-label="API request example">
      <div class="bar"><i></i><i></i><i></i></div>
      <pre><span class="c"># Get today's matches</span>
curl <span class="s">"https://api.example.com/api/v1/matches?per_page=5"</span> \
  -H <span class="s">"X-API-Key: m3_••••••••"</span>

<span class="c">// Response</span>
{
  <span class="k">"data"</span>: [ { <span class="k">"home"</span>: <span class="s">"MC Alger"</span>,
                      <span class="k">"away"</span>: <span class="s">"JS Kabylie"</span>,
                      <span class="k">"status"</span>: <span class="s">"finished"</span>,
                      <span class="k">"home_score"</span>: 2,
                      <span class="k">"away_score"</span>: 1 } ],
  <span class="k">"page"</span>: 1
}</pre>
    </div>
  </div>
  <div class="stats">
    <div class="stat"><b>3</b><span>competitions covered</span></div>
    <div class="stat"><b>3</b><span>seasons of archives</span></div>
    <div class="stat"><b>Plan limits</b><span>fair monthly quotas and rates</span></div>
  </div>
  <section>
    <div class="section-head"><h2>Everything your app needs</h2><p>One consistent JSON API across all three Algerian competitions.</p></div>
    <div class="grid">
      <article class="card"><span class="tag">SCHEDULE</span><h3>Fixtures &amp; results</h3><p>Filter by competition, season, round, club, date range and status.</p></article>
      <article class="card"><span class="tag">TABLES</span><h3>Standings</h3><p>Computed from results and grouped by division, with source links.</p></article>
      <article class="card"><span class="tag">NEAR LIVE</span><h3>Match status</h3><p>Polled official sources for current score, minute and freshness.</p></article>
      <article class="card"><span class="tag">EVENTS</span><h3>Match timeline</h3><p>Goals, cards and substitutions where published by the source.</p></article>
      <article class="card"><span class="tag">SEARCH</span><h3>Clubs &amp; matches</h3><p>Fast lookup across teams and fixtures with one search endpoint.</p></article>
      <article class="card"><span class="tag">CACHE</span><h3>Fast &amp; predictable</h3><p>Shared Redis cache and rate limits keep responses quick at load.</p></article>
    </div>
  </section>
  <section>
    <div class="section-head"><h2>Get your key in three steps</h2><p>No sales call. Self-service access from the dashboard.</p></div>
    <div class="steps">
      <div class="step"><h3>Create an account</h3><p>Register with your email and a password of at least 12 characters.</p></div>
      <div class="step"><h3>Generate a key</h3><p>Open the dashboard, name your key and copy it once.</p></div>
      <div class="step"><h3>Start building</h3><p>Send <code>X-API-Key</code> with every request. Revoke keys anytime.</p></div>
    </div>
  </section>
  <section class="cta">
    <div><h2>Ready to build?</h2><p>Request access and generate your first API key today.</p></div>
    <div class="actions" style="margin:0"><a class="btn btn-primary" href="/request-access">Request API key</a><a class="btn btn-ghost" href="/docs">Explore endpoints</a></div>
  </section>
</main>
<footer><span>© Matchs API · Algerian Ligue 1, Ligue 2 &amp; Inter-régions</span><span><a href="/healthz">Health</a> · <a href="/readyz">Readiness</a> · <a href="/dashboard">Dashboard</a></span></footer>
</div>
</body></html>"#,
    )
}

async fn docs() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Matchs API — Documentation</title>
<meta name="description" content="Reference for the Matchs API v1: authentication, endpoints, query parameters and examples.">
<style>
:root{--ink:#0f172a;--muted:#64748b;--teal:#0f766e;--line:#e2e8f0;--bg:#f8fafc}
*{box-sizing:border-box}
body{margin:0;font-family:Inter,ui-sans-serif,system-ui,-apple-system,'Segoe UI',sans-serif;color:var(--ink);background:radial-gradient(circle at 12% 0%,#ccfbf1 0,transparent 28%),var(--bg);line-height:1.65}
a{color:var(--teal);text-decoration:none}
a:hover{text-decoration:underline}
.wrap{max-width:1120px;margin:auto;padding:0 24px}
header{display:flex;align-items:center;justify-content:space-between;gap:16px;padding:20px 0;border-bottom:1px solid var(--line)}
.brand{display:flex;align-items:center;gap:10px;font-weight:800;letter-spacing:-.02em;color:var(--ink)}
.mark{width:32px;height:32px;border-radius:9px;background:linear-gradient(135deg,var(--teal),#14b8a6);display:grid;place-items:center;color:white;font-weight:900}
nav{display:flex;gap:20px;align-items:center;font-size:14px;color:var(--muted)}
.btn{display:inline-flex;align-items:center;justify-content:center;padding:10px 15px;border-radius:9px;background:var(--teal);color:white;font-weight:700;font-size:14px}
.btn:hover{background:#115e59;text-decoration:none}
.hero{padding:48px 0 30px}
.pill{display:inline-flex;align-items:center;gap:8px;padding:5px 11px;border-radius:999px;background:#ecfdf5;color:#047857;font-size:12px;font-weight:800;letter-spacing:.04em;text-transform:uppercase}
h1{font-size:46px;letter-spacing:-.04em;margin:16px 0 10px}
h2{font-size:26px;letter-spacing:-.03em;margin:0 0 14px}
h3{font-size:16px;margin:22px 0 8px}
p{margin:0 0 12px}
.lead{font-size:18px;color:var(--muted);max-width:680px}
.layout{display:grid;grid-template-columns:210px 1fr;gap:34px;align-items:start;padding-bottom:70px}
aside{position:sticky;top:20px;background:white;border:1px solid var(--line);border-radius:14px;padding:16px}
aside strong{display:block;font-size:12px;letter-spacing:.08em;color:var(--muted);text-transform:uppercase;margin:8px 0}
aside a{display:block;padding:6px 8px;border-radius:7px;font-size:14px;color:var(--ink)}
aside a:hover{background:#f1f5f9;text-decoration:none}
.card{background:white;border:1px solid var(--line);border-radius:16px;padding:24px;margin-bottom:18px;box-shadow:0 4px 16px #0f172a08}
.card>:last-child{margin-bottom:0}
code{background:#f1f5f9;padding:.15rem .35rem;border-radius:5px;font-size:13px}
pre{background:#020617;color:#e2e8f0;border-radius:11px;padding:16px;overflow:auto;font-size:13px;line-height:1.7;margin:12px 0}
pre code{background:none;padding:0;color:inherit}
.endpoint{border:1px solid var(--line);border-radius:11px;padding:13px 15px;margin:10px 0;background:#ffffff}
.endpoint-top{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.method{background:#ccfbf1;color:#115e59;border-radius:6px;padding:3px 8px;font-size:11px;font-weight:900;letter-spacing:.05em}
.path{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:14px;font-weight:700}
.endpoint p{color:var(--muted);font-size:14px;margin:8px 0 0}
table{width:100%;border-collapse:collapse;font-size:14px;margin-top:10px}
th,td{text-align:left;padding:9px 8px;border-bottom:1px solid var(--line);vertical-align:top}
th{color:var(--muted);font-size:12px;text-transform:uppercase;letter-spacing:.05em}
.note{border-left:3px solid var(--teal);background:#f0fdfa;padding:12px 14px;border-radius:0 9px 9px 0;color:#115e59;font-size:14px}
details{border:1px solid var(--line);border-radius:12px;background:white;padding:14px 16px}
summary{cursor:pointer;font-weight:700}
footer{border-top:1px solid var(--line);padding:24px 0 40px;color:var(--muted);font-size:13px;display:flex;justify-content:space-between;gap:16px;flex-wrap:wrap}
@media(max-width:860px){.layout{grid-template-columns:1fr}aside{position:static;display:flex;flex-wrap:wrap;gap:8px}aside strong{width:100%;margin:0}h1{font-size:38px}nav{gap:12px}}
</style>
</head>
<body>
<div class="wrap">
<header>
  <a class="brand" href="/"><span class="mark">M</span> Matchs API docs</a>
  <nav><a href="/">Home</a><a href="/request-access">Request key</a><a href="/dashboard">Dashboard</a><a class="btn" href='/openapi.json'>OpenAPI JSON</a></nav>
</header>
<main>
  <div class="hero"><span class="pill">API v1</span><h1>Documentation</h1><p class="lead">Everything you need to fetch Algerian fixtures, results, standings, events and near-live status from one JSON API.</p></div>
  <div class="layout">
    <aside>
      <strong>Reference</strong>
      <a href='#auth'>Authentication</a><a href='#quickstart'>Quick start</a><a href='#health'>Health</a>
      <strong>Data</strong>
      <a href='#competitions'>Competitions</a><a href='#matches'>Matches</a><a href='#teams'>Teams</a><a href='#standings'>Standings</a><a href='#search'>Search</a>
      <strong>Reference</strong>
      <a href='#errors'>Errors</a><a href='#limits'>Limits</a><a href='#spec'>Raw spec</a>
    </aside>
    <div>
      <section class="card" id="auth"><h2>Authentication</h2>
        <p>All <code>/api/v1/*</code> endpoints require an API key in the <code>X-API-Key</code> header. Create one from your dashboard after registering.</p>
        <pre><code>curl 'https://api.example.com/api/v1/matches?per_page=5' \
  -H 'X-API-Key: m3_your_key_here'</code></pre>
        <div class="note">Never put API keys in a browser URL or public repository. Keys can be revoked at any time from the dashboard.</div>
      </section>
      <section class="card" id="quickstart"><h2>Quick start</h2>
        <p>List upcoming Ligue 1 matches for the current season:</p>
        <pre><code>curl 'https://api.example.com/api/v1/matches?competition=L1&amp;status=scheduled&amp;per_page=10' \
  -H 'X-API-Key: m3_your_key_here'</code></pre>
        <p>Every list endpoint returns a paginated object with <code>data</code>, <code>page</code>, <code>per_page</code> and <code>total</code>.</p>
        <h3>JavaScript</h3>
        <pre><code>const r = await fetch('https://api.example.com/api/v1/matches?per_page=5', {
  headers: {'X-API-Key': 'm3_your_key_here'}
});
const data = await r.json();</code></pre>
        <h3>Python</h3>
        <pre><code>import requests
r = requests.get('https://api.example.com/api/v1/matches',
                 headers={'X-API-Key': 'm3_your_key_here'},
                 params={'per_page': 5})
data = r.json()</code></pre>
      </section>
      <section class="card" id="health"><h2>Health</h2>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/healthz</span></div><p>Liveness probe. Returns <code>200 ok</code>. No API key required.</p></div>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/readyz</span></div><p>Readiness probe. Checks the database and cache. No API key required.</p></div>
      </section>
      <section class="card" id="competitions"><h2>Competitions</h2>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/competitions</span></div><p>List available competitions: Ligue 1, Ligue 2 and Inter-régions.</p></div>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/seasons</span></div><p>List seasons, optionally filtered by <code>competition</code>.</p></div>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/groups</span></div><p>List groups/divisions used by standings and fixtures.</p></div>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/clubs</span></div><p>Search clubs by name with <code>?q=</code> and <code>?limit=</code>.</p></div>
      </section>
      <section class="card" id="matches"><h2>Matches</h2>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/matches</span></div><p>Filter and paginate matches. All parameters are optional.</p></div>
        <table><thead><tr><th>Parameter</th><th>Example</th><th>Description</th></tr></thead><tbody>
          <tr><td><code>competition</code></td><td><code>L1</code></td><td>Competition code.</td></tr>
          <tr><td><code>season</code></td><td><code>lfp:6</code></td><td>Season identifier.</td></tr>
          <tr><td><code>round</code></td><td><code>12</code></td><td>Matchday number.</td></tr>
          <tr><td><code>status</code></td><td><code>finished</code></td><td>Match status.</td></tr>
          <tr><td><code>from</code> / <code>to</code></td><td><code>2026-11-01T00:00:00Z</code></td><td>Kickoff date range.</td></tr>
          <tr><td><code>club_id</code></td><td><code>uuid</code></td><td>Only matches for one club.</td></tr>
          <tr><td><code>page</code> / <code>per_page</code></td><td><code>1</code> / <code>20</code></td><td>Pagination; <code>per_page</code> max is 100.</td></tr>
        </tbody></table>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/matches/{id}</span></div><p>One match with score, venue, status, source and source URL.</p></div>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/matches/{id}/events</span></div><p>Goal, card and substitution events when the source publishes them.</p></div>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/live</span></div><p>Matches currently marked live or near-live from the official source poll.</p></div>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/status</span></div><p>Account plan/quota/usage, source freshness, and ingest health: last success/failure, stuck runs, counters and the 10 most recent runs.</p></div>
        <div class="note">Matches include <code>last_polled_at</code>, <code>source_updated_at</code> and <code>stale</code>. <code>stale</code> is true only when a live/near-live match has not been polled recently. <code>ingest.healthy</code> is false after <code>INGEST_ALERT_AFTER_SECS</code> without a successful run, after a newer failure, or when a run appears stuck.</div>
      </section>
      <section class="card" id="teams"><h2>Teams</h2>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/teams/{id}/form</span></div><p>Last finished matches for one club with W/D/L result, opponent, venue and goals. Optional <code>season</code> and <code>limit</code> (1-20, default 5).</p></div>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/teams/{id}/head-to-head?opponent={uuid}</span></div><p>Head-to-head record from up to 500 most recent meetings and a shorter match list. Optional <code>season</code> and <code>limit</code> (1-50, default 10).</p></div>
      </section>
      <section class="card" id="standings"><h2>Standings</h2>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/standings</span></div><p>League table computed from results. Filter by <code>season</code>, <code>group</code> and <code>competition</code>; add <code>scope=home</code> or <code>scope=away</code> for split tables.</p></div>
      </section>
      <section class="card" id="search"><h2>Search</h2>
        <div class="endpoint"><div class="endpoint-top"><span class="method">GET</span><span class="path">/api/v1/search</span></div><p>Search clubs and matches with a single <code>q</code> parameter of 1-100 characters.</p></div>
      </section>
      <section class="card" id="errors"><h2>Errors &amp; limits</h2>
        <table><thead><tr><th>Plan</th><th>Monthly quota</th><th>Rate limit</th></tr></thead><tbody>
          <tr><td><code>free</code></td><td>10,000</td><td>60/min</td></tr>
          <tr><td><code>pro</code></td><td>1,000,000</td><td>300/min</td></tr>
          <tr><td><code>business</code></td><td>10,000,000</td><td>1,000/min</td></tr>
        </tbody></table>
        <table><thead><tr><th>Status</th><th>Meaning</th></tr></thead><tbody>
          <tr><td><code>400</code></td><td>Invalid parameter or missing CSRF token on a dashboard form.</td></tr>
          <tr><td><code>401</code></td><td>Missing or invalid API key/session.</td></tr>
          <tr><td><code>403</code></td><td>Key lacks the required scope.</td></tr>
          <tr><td><code>404</code></td><td>Resource not found.</td></tr>
          <tr><td><code>429</code></td><td>Rate or quota limit exceeded. Slow down and retry.</td></tr>
          <tr><td><code>500</code></td><td>Unexpected server error.</td></tr>
        </tbody></table>
        <p id="limits" class="note">Monthly quota and rate limit come from your account plan and are shared across all keys. Responses include <code>X-Request-Id</code>, <code>X-RateLimit-Limit</code>, <code>X-RateLimit-Remaining</code> and <code>X-Requests-Remaining</code>. Data responses include <code>ETag</code> and <code>Cache-Control</code>; send <code>If-None-Match</code> for a <code>304</code>.</p>
      </section>
      <section id="spec"><details><summary>Raw OpenAPI specification</summary><pre id="spec-json">Loading…</pre></details></section>
    </div>
  </div>
</main>
<footer><span>© Matchs API · Algerian Ligue 1, Ligue 2 &amp; Inter-régions</span><span><a href='/openapi.json'>Download spec</a> · <a href='/'>Home</a></span></footer>
</div>
<script>fetch('/openapi.json').then(function(r){return r.json()}).then(function(j){document.getElementById('spec-json').textContent=JSON.stringify(j,null,2)}).catch(function(){document.getElementById('spec-json').textContent='Failed to load /openapi.json'})</script>
</body></html>"#,
    )
}

async fn openapi() -> Json<Value> {
    Json(serde_json::json!({
        "openapi": "3.0.3",
        "info": {"title": "Algerian Matches API", "version": "1.4.0"},
        "servers": [{"url": "/"}],
        "components": {
            "securitySchemes": {"ApiKey": {"type": "apiKey", "in": "header", "name": "X-API-Key"}},
            "headers": {
                "X-Request-Id": {"schema": {"type": "string"}},
                "X-RateLimit-Limit": {"schema": {"type": "integer"}},
                "X-RateLimit-Remaining": {"schema": {"type": "integer"}},
                "X-Requests-Remaining": {"schema": {"type": "integer"}},
                "ETag": {"schema": {"type": "string"}},
                "Cache-Control": {"schema": {"type": "string"}}
            },
            "schemas": {
                "Club": {"type": "object", "properties": {
                    "id": {"type": "string", "format": "uuid"},
                    "name": {"type": "string"},
                    "short_name": {"type": "string"},
                    "logo_url": {"type": "string", "nullable": true}
                }},
                "Match": {"type": "object", "properties": {
                    "id": {"type": "string", "format": "uuid"},
                    "competition_code": {"type": "string", "example": "L1"},
                    "season_id": {"type": "string", "example": "lfp:6"},
                    "round": {"type": "integer"},
                    "kickoff_utc": {"type": "string", "format": "date-time"},
                    "status": {"type": "string", "example": "finished"},
                    "home_score": {"type": "integer", "nullable": true},
                    "away_score": {"type": "integer", "nullable": true},
                    "is_live": {"type": "boolean"},
                    "live_minute": {"type": "string", "nullable": true},
                    "source": {"type": "string"},
                    "source_url": {"type": "string"},
                    "updated_at": {"type": "string", "format": "date-time"},
                    "last_polled_at": {"type": "string", "format": "date-time"},
                    "source_updated_at": {"type": "string", "format": "date-time", "nullable": true},
                    "stale": {"type": "boolean", "description": "True when a live match was not polled recently."}
                }},
                "Standing": {"type": "object"},
                "FormOutcome": {"type": "object", "properties": {
                    "match_id": {"type": "string", "format": "uuid"},
                    "kickoff_utc": {"type": "string", "format": "date-time"},
                    "opponent": {"$ref": "#/components/schemas/Club"},
                    "venue": {"type": "string", "enum": ["home", "away"]},
                    "result": {"type": "string", "enum": ["W", "D", "L"]},
                    "goals_for": {"type": "integer"},
                    "goals_against": {"type": "integer"}
                }},
                "TeamForm": {"type": "object", "properties": {
                    "club": {"$ref": "#/components/schemas/Club"},
                    "played": {"type": "integer"},
                    "wins": {"type": "integer"},
                    "draws": {"type": "integer"},
                    "losses": {"type": "integer"},
                    "goals_for": {"type": "integer"},
                    "goals_against": {"type": "integer"},
                    "form": {"type": "array", "items": {"$ref": "#/components/schemas/FormOutcome"}}
                }},
                "HeadToHead": {"type": "object", "properties": {
                    "club": {"$ref": "#/components/schemas/Club"},
                    "opponent": {"$ref": "#/components/schemas/Club"},
                    "played": {"type": "integer"},
                    "club_wins": {"type": "integer"},
                    "draws": {"type": "integer"},
                    "opponent_wins": {"type": "integer"},
                    "club_goals": {"type": "integer"},
                    "opponent_goals": {"type": "integer"},
                    "matches": {"type": "array", "items": {"$ref": "#/components/schemas/Match"}}
                }},
                "Page": {"type": "object", "properties": {
                    "data": {"type": "array", "items": {"$ref": "#/components/schemas/Match"}},
                    "page": {"type": "integer"},
                    "per_page": {"type": "integer"},
                    "total": {"type": "integer"}
                }},
                "AccountStatus": {"type": "object", "properties": {
                    "account_id": {"type": "string", "format": "uuid"},
                    "key_id": {"type": "string", "format": "uuid"},
                    "key_name": {"type": "string"},
                    "plan": {"type": "string"},
                    "scopes": {"type": "array", "items": {"type": "string"}},
                    "quota": {"type": "integer"},
                    "requests_used": {"type": "integer"},
                    "requests_remaining": {"type": "integer"},
                    "rate_limit_per_min": {"type": "integer"},
                    "rate_remaining": {"type": "integer"}
                }},
                "StatusRow": {"type": "object", "properties": {
                    "source": {"type": "string"},
                    "matches": {"type": "integer"},
                    "last_match": {"type": "string", "format": "date-time", "nullable": true},
                    "live": {"type": "integer"},
                    "last_polled_at": {"type": "string", "format": "date-time", "nullable": true},
                    "stale_matches": {"type": "integer"}
                }},
                "IngestRun": {"type": "object", "properties": {
                    "mode": {"type": "string", "enum": ["backfill", "live", "events"]},
                    "source": {"type": "string"},
                    "status": {"type": "string", "enum": ["running", "success", "failed"]},
                    "started_at": {"type": "string", "format": "date-time"},
                    "finished_at": {"type": "string", "format": "date-time", "nullable": true},
                    "duration_ms": {"type": "integer", "nullable": true},
                    "matches_seen": {"type": "integer"},
                    "matches_upserted": {"type": "integer"},
                    "events_upserted": {"type": "integer"},
                    "standings_rebuilt": {"type": "integer"},
                    "error": {"type": "string", "nullable": true}
                }},
                "IngestStatus": {"type": "object", "properties": {
                    "healthy": {"type": "boolean"},
                    "alert_after_secs": {"type": "integer"},
                    "last_success_at": {"type": "string", "format": "date-time", "nullable": true},
                    "last_failure_at": {"type": "string", "format": "date-time", "nullable": true},
                    "stuck_runs": {"type": "integer"},
                    "recent": {"type": "array", "items": {"$ref": "#/components/schemas/IngestRun"}}
                }},
                "StatusResponse": {"type": "object", "properties": {
                    "account": {"$ref": "#/components/schemas/AccountStatus"},
                    "sources": {"type": "array", "items": {"$ref": "#/components/schemas/StatusRow"}},
                    "ingest": {"$ref": "#/components/schemas/IngestStatus"}
                }}
            }
        },
        "security": [{"ApiKey": []}],
        "paths": {
            "/healthz": {"get": {"summary": "Liveness", "security": [], "responses": {"200": {"description": "OK"}}}},
            "/readyz": {"get": {"summary": "Readiness", "security": [], "responses": {"200": {"description": "OK"}}}},
            "/api/v1/competitions": {"get": {"summary": "List competitions", "responses": {"200": {"description": "OK"}}}},
            "/api/v1/seasons": {"get": {"summary": "List seasons", "parameters": [{"name": "competition", "in": "query", "schema": {"type": "string"}}], "responses": {"200": {"description": "OK"}}}},
            "/api/v1/groups": {"get": {"summary": "List groups", "responses": {"200": {"description": "OK"}}}},
            "/api/v1/clubs": {"get": {"summary": "Search clubs", "responses": {"200": {"description": "OK"}}}},
            "/api/v1/matches": {"get": {"summary": "List matches", "parameters": [
                {"name": "competition", "in": "query", "schema": {"type": "string"}},
                {"name": "season", "in": "query", "schema": {"type": "string"}},
                {"name": "round", "in": "query", "schema": {"type": "integer"}},
                {"name": "status", "in": "query", "schema": {"type": "string"}},
                {"name": "from", "in": "query", "schema": {"type": "string", "format": "date-time"}},
                {"name": "to", "in": "query", "schema": {"type": "string", "format": "date-time"}},
                {"name": "club_id", "in": "query", "schema": {"type": "string", "format": "uuid"}},
                {"name": "page", "in": "query", "schema": {"type": "integer"}},
                {"name": "per_page", "in": "query", "schema": {"type": "integer", "maximum": 100}}
            ], "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Page"}}}, "headers": {
                "X-Request-Id": {"$ref": "#/components/headers/X-Request-Id"},
                "X-RateLimit-Limit": {"$ref": "#/components/headers/X-RateLimit-Limit"},
                "X-RateLimit-Remaining": {"$ref": "#/components/headers/X-RateLimit-Remaining"},
                "X-Requests-Remaining": {"$ref": "#/components/headers/X-Requests-Remaining"},
                "ETag": {"$ref": "#/components/headers/ETag"},
                "Cache-Control": {"$ref": "#/components/headers/Cache-Control"}
            }}}}},
            "/api/v1/matches/{id}": {"get": {"summary": "Get match", "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Match"}}}}, "404": {"description": "Not found"}}}},
            "/api/v1/matches/{id}/events": {"get": {"summary": "Get match events", "responses": {"200": {"description": "OK"}}}},
            "/api/v1/teams/{id}/form": {"get": {"summary": "Recent team form", "parameters": [
                {"name": "id", "in": "path", "required": true, "schema": {"type": "string", "format": "uuid"}},
                {"name": "season", "in": "query", "schema": {"type": "string"}},
                {"name": "limit", "in": "query", "schema": {"type": "integer", "minimum": 1, "maximum": 20, "default": 5}}
            ], "responses": {
                "200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TeamForm"}}}},
                "404": {"description": "Club not found"}
            }}},
            "/api/v1/teams/{id}/head-to-head": {"get": {"summary": "Head-to-head record", "parameters": [
                {"name": "id", "in": "path", "required": true, "schema": {"type": "string", "format": "uuid"}},
                {"name": "opponent", "in": "query", "required": true, "schema": {"type": "string", "format": "uuid"}},
                {"name": "season", "in": "query", "schema": {"type": "string"}},
                {"name": "limit", "in": "query", "schema": {"type": "integer", "minimum": 1, "maximum": 50, "default": 10}}
            ], "responses": {
                "200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/HeadToHead"}}}},
                "400": {"description": "Invalid opponent"},
                "404": {"description": "Club not found"}
            }}},
            "/api/v1/live": {"get": {"summary": "Near-live matches", "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": {"type": "array", "items": {"$ref": "#/components/schemas/Match"}}}}}}}},
            "/api/v1/standings": {"get": {"summary": "Standings", "parameters": [
                {"name": "season", "in": "query", "required": true, "schema": {"type": "string"}},
                {"name": "group", "in": "query", "schema": {"type": "string"}},
                {"name": "competition", "in": "query", "schema": {"type": "string"}},
                {"name": "scope", "in": "query", "schema": {"type": "string", "enum": ["all", "home", "away"], "default": "all"}}
            ], "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": {"type": "array", "items": {"$ref": "#/components/schemas/Standing"}}}}}}}},
            "/api/v1/search": {"get": {"summary": "Search clubs and matches", "responses": {"200": {"description": "OK"}}}},
            "/api/v1/status": {"get": {"summary": "Account usage, source freshness and ingest health", "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/StatusResponse"}}}}}}}
        }
    }))
}

async fn cached<T, F>(app: &AppState, key: String, future: F) -> ApiResult<Json<T>>
where
    T: Serialize + for<'de> Deserialize<'de>,
    F: Future<Output = ApiResult<T>>,
{
    if let Some(value) = app.cache.get(&key).await {
        if let Ok(parsed) = serde_json::from_str::<T>(&value) {
            return Ok(Json(parsed));
        }
    }
    let value = future.await?;
    if let Ok(serialized) = serde_json::to_string(&value) {
        app.cache
            .set(
                key,
                serialized,
                Duration::from_secs(app.config.cache_ttl_secs),
            )
            .await;
    }
    Ok(Json(value))
}

#[derive(Debug, FromRow, Serialize, Deserialize)]
struct CompetitionRow {
    code: String,
    name: String,
    tier: i32,
    source: String,
}

impl From<CompetitionRow> for Competition {
    fn from(row: CompetitionRow) -> Self {
        Self {
            code: row.code,
            name: row.name,
            tier: row.tier,
            source: row.source,
        }
    }
}

async fn competitions(
    State(app): State<AppState>,
    auth: AuthContext,
) -> ApiResult<Json<Vec<Competition>>> {
    auth.require("read:matches")?;
    cached(&app, "api:competitions".into(), async {
        let rows: Vec<CompetitionRow> =
            sqlx::query_as("SELECT code, name, tier, source FROM competitions ORDER BY tier")
                .fetch_all(&app.pool)
                .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    })
    .await
}

#[derive(Debug, Deserialize)]
struct SeasonQuery {
    competition: Option<String>,
}

#[derive(Debug, FromRow, Serialize, Deserialize)]
struct SeasonRow {
    id: String,
    competition_code: String,
    label: String,
}

async fn seasons(
    State(app): State<AppState>,
    Query(query): Query<SeasonQuery>,
    auth: AuthContext,
) -> ApiResult<Json<Vec<Season>>> {
    auth.require("read:matches")?;
    let key = format!(
        "api:seasons:{}",
        query.competition.as_deref().unwrap_or("all")
    );
    cached(&app, key, async {
        let mut builder =
            QueryBuilder::new("SELECT id, competition_code, label FROM seasons WHERE 1=1");
        if let Some(code) = &query.competition {
            builder.push(" AND competition_code = ").push_bind(code);
        }
        builder.push(" ORDER BY competition_code, source_season_id DESC");
        let rows: Vec<SeasonRow> = builder.build_query_as().fetch_all(&app.pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| Season {
                id: r.id,
                competition_code: r.competition_code,
                label: r.label,
            })
            .collect())
    })
    .await
}

#[derive(Debug, Deserialize)]
struct GroupQuery {
    competition: Option<String>,
    season: Option<String>,
}

#[derive(Debug, FromRow, Serialize, Deserialize)]
struct GroupRow {
    id: String,
    competition_code: String,
    name: String,
}

async fn groups(
    State(app): State<AppState>,
    Query(query): Query<GroupQuery>,
    auth: AuthContext,
) -> ApiResult<Json<Vec<FootballGroup>>> {
    auth.require("read:matches")?;
    let key = format!(
        "api:groups:{}:{}",
        query.competition.as_deref().unwrap_or("all"),
        query.season.as_deref().unwrap_or("all")
    );
    cached(&app, key, async {
        let mut builder = QueryBuilder::new(
            "SELECT id, competition_code, name FROM groups WHERE 1=1",
        );
        if let Some(code) = &query.competition {
            builder.push(" AND competition_code = ").push_bind(code);
        }
        if let Some(season) = &query.season {
            builder.push(" AND EXISTS (SELECT 1 FROM matches m WHERE m.group_id = groups.id AND m.season_id = ")
                .push_bind(season)
                .push(")");
        }
        builder.push(" ORDER BY competition_code, name");
        let rows: Vec<GroupRow> = builder.build_query_as().fetch_all(&app.pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| FootballGroup { id: r.id, competition_code: r.competition_code, name: r.name })
            .collect())
    })
    .await
}

#[derive(Debug, FromRow, Serialize, Deserialize, Clone)]
struct MatchRow {
    id: Uuid,
    competition_code: String,
    season_id: String,
    group_id: Option<String>,
    group_name: Option<String>,
    round: i32,
    kickoff_utc: DateTime<Utc>,
    h_id: Uuid,
    h_name: String,
    h_short: String,
    h_logo: Option<String>,
    a_id: Uuid,
    a_name: String,
    a_short: String,
    a_logo: Option<String>,
    home_score: Option<i32>,
    away_score: Option<i32>,
    status: String,
    venue: Option<String>,
    is_live: bool,
    live_minute: Option<String>,
    source: String,
    source_url: Option<String>,
    updated_at: DateTime<Utc>,
    last_polled_at: DateTime<Utc>,
    source_updated_at: Option<DateTime<Utc>>,
}

impl MatchRow {
    fn into_match(self, stale_after_secs: u64) -> Match {
        let stale = self.is_live
            && Utc::now()
                .signed_duration_since(self.last_polled_at)
                .num_seconds()
                > stale_after_secs as i64;
        Match {
            id: self.id,
            competition_code: self.competition_code,
            season_id: self.season_id,
            group_id: self.group_id,
            group_name: self.group_name,
            round: self.round,
            kickoff_utc: self.kickoff_utc,
            home: Club {
                id: self.h_id,
                name: self.h_name,
                short_name: self.h_short,
                logo_url: self.h_logo,
            },
            away: Club {
                id: self.a_id,
                name: self.a_name,
                short_name: self.a_short,
                logo_url: self.a_logo,
            },
            home_score: self.home_score,
            away_score: self.away_score,
            status: self.status,
            venue: self.venue,
            is_live: self.is_live,
            live_minute: self.live_minute,
            source: self.source,
            source_url: self.source_url.unwrap_or_default(),
            updated_at: self.updated_at,
            last_polled_at: self.last_polled_at,
            source_updated_at: self.source_updated_at,
            stale,
        }
    }
}

const MATCH_SELECT: &str = "SELECT m.id, m.competition_code, m.season_id, m.group_id, g.name AS group_name,
    m.round, m.kickoff_utc,
    h.id AS h_id, h.canonical_name AS h_name, h.short_name AS h_short, h.logo_url AS h_logo,
    a.id AS a_id, a.canonical_name AS a_name, a.short_name AS a_short, a.logo_url AS a_logo,
    m.home_score, m.away_score, m.status, m.venue, m.is_live, m.live_minute, m.source, m.source_url, m.updated_at,
    m.last_polled_at, m.source_updated_at
    FROM matches m
    JOIN clubs h ON h.id = m.home_club_id
    JOIN clubs a ON a.id = m.away_club_id
    LEFT JOIN groups g ON g.id = m.group_id";

#[derive(Debug, Deserialize)]
struct MatchQuery {
    competition: Option<String>,
    season: Option<String>,
    group: Option<String>,
    round: Option<i32>,
    status: Option<String>,
    from: Option<String>,
    to: Option<String>,
    club_id: Option<Uuid>,
    page: Option<u32>,
    per_page: Option<u32>,
}

async fn matches(
    State(app): State<AppState>,
    Query(query): Query<MatchQuery>,
    auth: AuthContext,
) -> ApiResult<Json<Page<Match>>> {
    auth.require("read:matches")?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let key = format!(
        "api:matches:{:?}",
        (
            &query.competition,
            &query.season,
            &query.group,
            query.round,
            &query.status,
            &query.from,
            &query.to,
            query.club_id,
            page,
            per_page
        )
    );
    cached(&app, key, async {
        let from = parse_dt(query.from.as_deref())?;
        let to = parse_dt(query.to.as_deref())?;

        #[derive(Debug, FromRow)]
        struct CountRow {
            count: i64,
        }

        let mut count_builder = QueryBuilder::new("SELECT COUNT(*) AS count FROM matches m ");
        apply_match_filters(&mut count_builder, &query, from, to);
        let count: CountRow = count_builder.build_query_as().fetch_one(&app.pool).await?;

        let mut data_builder = QueryBuilder::new(MATCH_SELECT);
        data_builder.push(" ");
        apply_match_filters(&mut data_builder, &query, from, to);
        data_builder
            .push(" ORDER BY m.kickoff_utc DESC LIMIT ")
            .push_bind(per_page as i64)
            .push(" OFFSET ")
            .push_bind(((page - 1) * per_page) as i64);
        let rows: Vec<MatchRow> = data_builder.build_query_as().fetch_all(&app.pool).await?;
        Ok(Page {
            data: rows
                .into_iter()
                .map(|row| row.into_match(app.config.stale_after_secs))
                .collect(),
            page,
            per_page,
            total: count.count,
        })
    })
    .await
}

fn apply_match_filters<'a>(
    builder: &mut QueryBuilder<'a, sqlx::Postgres>,
    query: &'a MatchQuery,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) {
    builder.push("WHERE 1=1");
    if let Some(v) = &query.competition {
        builder.push(" AND m.competition_code = ").push_bind(v);
    }
    if let Some(v) = &query.season {
        builder.push(" AND m.season_id = ").push_bind(v);
    }
    if let Some(v) = &query.group {
        builder.push(" AND m.group_id = ").push_bind(v);
    }
    if let Some(v) = query.round {
        builder.push(" AND m.round = ").push_bind(v);
    }
    if let Some(v) = &query.status {
        builder.push(" AND m.status = ").push_bind(v);
    }
    if let Some(v) = from {
        builder.push(" AND m.kickoff_utc >= ").push_bind(v);
    }
    if let Some(v) = to {
        builder.push(" AND m.kickoff_utc <= ").push_bind(v);
    }
    if let Some(v) = query.club_id {
        builder
            .push(" AND (m.home_club_id = ")
            .push_bind(v)
            .push(" OR m.away_club_id = ")
            .push_bind(v)
            .push(")");
    }
}

fn parse_dt(value: Option<&str>) -> ApiResult<Option<DateTime<Utc>>> {
    let Some(value) = value else { return Ok(None) };
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|_| ApiError::BadRequest(format!("invalid date-time: {value}")))?;
    Ok(Some(parsed.with_timezone(&Utc)))
}

async fn match_by_id(
    State(app): State<AppState>,
    Path(id): Path<Uuid>,
    auth: AuthContext,
) -> ApiResult<Json<Match>> {
    auth.require("read:matches")?;
    let row: MatchRow = sqlx::query_as(&format!("{MATCH_SELECT} WHERE m.id = $1"))
        .bind(id)
        .fetch_optional(&app.pool)
        .await?
        .ok_or_else(|| ApiError::NotFound("match not found".into()))?;
    Ok(Json(row.into_match(app.config.stale_after_secs)))
}

#[derive(Debug, FromRow, Serialize, Deserialize)]
struct EventRow {
    id: i64,
    minute: Option<i32>,
    kind: String,
    team_side: Option<String>,
    player: Option<String>,
    detail: Option<String>,
}

async fn match_events(
    State(app): State<AppState>,
    Path(id): Path<Uuid>,
    auth: AuthContext,
) -> ApiResult<Json<Vec<MatchEvent>>> {
    auth.require("read:matches")?;
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, minute, kind, team_side, player, detail FROM match_events WHERE match_id = $1 ORDER BY minute NULLS LAST, id",
    )
    .bind(id)
    .fetch_all(&app.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| MatchEvent {
                id: r.id,
                minute: r.minute,
                kind: r.kind,
                team_side: r.team_side,
                player: r.player,
                detail: r.detail,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
struct TeamFormQuery {
    season: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct HeadToHeadQuery {
    opponent: Uuid,
    season: Option<String>,
    limit: Option<i64>,
}

async fn club_by_id(app: &AppState, id: Uuid) -> ApiResult<Club> {
    let row: ClubRow =
        sqlx::query_as("SELECT id, canonical_name, short_name, logo_url FROM clubs WHERE id = $1")
            .bind(id)
            .fetch_optional(&app.pool)
            .await?
            .ok_or_else(|| ApiError::NotFound("club not found".into()))?;
    Ok(Club {
        id: row.id,
        name: row.canonical_name,
        short_name: row.short_name,
        logo_url: row.logo_url,
    })
}

fn outcome(goals_for: i32, goals_against: i32) -> &'static str {
    match goals_for.cmp(&goals_against) {
        std::cmp::Ordering::Greater => "W",
        std::cmp::Ordering::Less => "L",
        std::cmp::Ordering::Equal => "D",
    }
}

fn form_outcome(match_: &Match, team_id: Uuid) -> FormOutcome {
    let is_home = match_.home.id == team_id;
    let goals_for = if is_home {
        match_.home_score.unwrap_or_default()
    } else {
        match_.away_score.unwrap_or_default()
    };
    let goals_against = if is_home {
        match_.away_score.unwrap_or_default()
    } else {
        match_.home_score.unwrap_or_default()
    };
    FormOutcome {
        match_id: match_.id,
        kickoff_utc: match_.kickoff_utc,
        opponent: if is_home {
            match_.away.clone()
        } else {
            match_.home.clone()
        },
        venue: if is_home { "home" } else { "away" }.to_string(),
        result: outcome(goals_for, goals_against).to_string(),
        goals_for,
        goals_against,
    }
}

async fn team_form(
    State(app): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<TeamFormQuery>,
    auth: AuthContext,
) -> ApiResult<Json<TeamForm>> {
    auth.require("read:matches")?;
    let limit = query.limit.unwrap_or(5).clamp(1, 20);
    let key = format!(
        "api:team-form:{id}:{}:{limit}",
        query.season.as_deref().unwrap_or("all")
    );
    let Json(matches) = cached(&app, key, async {
        let rows: Vec<MatchRow> = sqlx::query_as(&format!(
            "{MATCH_SELECT} WHERE (m.home_club_id = $1 OR m.away_club_id = $1)
             AND ($2::text IS NULL OR m.season_id = $2)
             AND m.home_score IS NOT NULL AND m.away_score IS NOT NULL
             AND m.status = 'finished'
             ORDER BY m.kickoff_utc DESC LIMIT $3"
        ))
        .bind(id)
        .bind(query.season.as_deref())
        .bind(limit)
        .fetch_all(&app.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.into_match(app.config.stale_after_secs))
            .collect::<Vec<Match>>())
    })
    .await?;
    let club = club_by_id(&app, id).await?;
    let form: Vec<FormOutcome> = matches
        .iter()
        .map(|match_| form_outcome(match_, id))
        .collect();
    let mut wins = 0;
    let mut draws = 0;
    let mut losses = 0;
    let mut goals_for = 0;
    let mut goals_against = 0;
    for item in &form {
        match item.result.as_str() {
            "W" => wins += 1,
            "D" => draws += 1,
            _ => losses += 1,
        }
        goals_for += item.goals_for;
        goals_against += item.goals_against;
    }
    Ok(Json(TeamForm {
        club,
        played: form.len() as i32,
        wins,
        draws,
        losses,
        goals_for,
        goals_against,
        form,
    }))
}

async fn head_to_head(
    State(app): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<HeadToHeadQuery>,
    auth: AuthContext,
) -> ApiResult<Json<HeadToHead>> {
    auth.require("read:matches")?;
    if id == query.opponent {
        return Err(ApiError::BadRequest("opponent must be another club".into()));
    }
    let limit = query.limit.unwrap_or(10).clamp(1, 50);
    let key = format!(
        "api:h2h:{id}:{}:{}:{limit}",
        query.opponent,
        query.season.as_deref().unwrap_or("all")
    );
    let Json(rows) = cached(&app, key, async {
        let rows: Vec<MatchRow> = sqlx::query_as(&format!(
            "{MATCH_SELECT} WHERE ((m.home_club_id = $1 AND m.away_club_id = $2)
                OR (m.home_club_id = $2 AND m.away_club_id = $1))
             AND ($3::text IS NULL OR m.season_id = $3)
             AND m.home_score IS NOT NULL AND m.away_score IS NOT NULL
             AND m.status = 'finished'
             ORDER BY m.kickoff_utc DESC LIMIT 500"
        ))
        .bind(id)
        .bind(query.opponent)
        .bind(query.season.as_deref())
        .fetch_all(&app.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.into_match(app.config.stale_after_secs))
            .collect::<Vec<Match>>())
    })
    .await?;
    let club = club_by_id(&app, id).await?;
    let opponent = club_by_id(&app, query.opponent).await?;
    let mut club_wins = 0;
    let mut draws = 0;
    let mut opponent_wins = 0;
    let mut club_goals = 0;
    let mut opponent_goals = 0;
    for match_ in &rows {
        let club_is_home = match_.home.id == id;
        let scored = if club_is_home {
            match_.home_score.unwrap_or_default()
        } else {
            match_.away_score.unwrap_or_default()
        };
        let conceded = if club_is_home {
            match_.away_score.unwrap_or_default()
        } else {
            match_.home_score.unwrap_or_default()
        };
        match scored.cmp(&conceded) {
            std::cmp::Ordering::Greater => club_wins += 1,
            std::cmp::Ordering::Less => opponent_wins += 1,
            std::cmp::Ordering::Equal => draws += 1,
        }
        club_goals += scored;
        opponent_goals += conceded;
    }
    Ok(Json(HeadToHead {
        club,
        opponent,
        played: rows.len() as i32,
        club_wins,
        draws,
        opponent_wins,
        club_goals,
        opponent_goals,
        matches: rows.into_iter().take(limit as usize).collect(),
    }))
}

async fn live_matches(
    State(app): State<AppState>,
    auth: AuthContext,
) -> ApiResult<Json<Vec<Match>>> {
    auth.require("read:matches")?;
    cached(&app, "api:live".into(), async {
        let rows: Vec<MatchRow> = sqlx::query_as(&format!(
            "{MATCH_SELECT} WHERE m.is_live = TRUE OR m.status = 'live' ORDER BY m.kickoff_utc DESC LIMIT 200"
        ))
        .fetch_all(&app.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.into_match(app.config.stale_after_secs))
            .collect())
    })
    .await
}

#[derive(Debug, Deserialize)]
struct StandingQuery {
    season: String,
    group: Option<String>,
    competition: Option<String>,
    scope: Option<String>,
}

#[derive(Debug, FromRow, Serialize, Deserialize)]
struct StandingRow {
    club_id: Uuid,
    club_name: String,
    club_short: String,
    club_logo: Option<String>,
    position: i32,
    played: i32,
    wins: i32,
    draws: i32,
    losses: i32,
    goals_for: i32,
    goals_against: i32,
    goal_difference: i32,
    points: i32,
}

#[derive(Debug, Clone, FromRow)]
struct StandingSourceRow {
    home_id: Uuid,
    home_name: String,
    home_short: String,
    home_logo: Option<String>,
    away_id: Uuid,
    away_name: String,
    away_short: String,
    away_logo: Option<String>,
    home_score: i32,
    away_score: i32,
}

#[derive(Debug, Default, Clone, Copy)]
struct StandingStats {
    played: i32,
    wins: i32,
    draws: i32,
    losses: i32,
    goals_for: i32,
    goals_against: i32,
}

impl StandingStats {
    fn points(self) -> i32 {
        self.wins * 3 + self.draws
    }

    fn goal_difference(self) -> i32 {
        self.goals_for - self.goals_against
    }
}

fn standings_scope(value: Option<&str>) -> ApiResult<&str> {
    match value.unwrap_or("all") {
        "all" | "home" | "away" => Ok(value.unwrap_or("all")),
        other => Err(ApiError::BadRequest(format!(
            "invalid standings scope: {other}"
        ))),
    }
}

fn side_standings(rows: Vec<StandingSourceRow>, scope: &str) -> Vec<Standing> {
    let mut table: HashMap<Uuid, (Club, StandingStats)> = HashMap::new();
    for row in rows {
        let home = scope == "home";
        let id = if home { row.home_id } else { row.away_id };
        let club = Club {
            id,
            name: if home { row.home_name } else { row.away_name },
            short_name: if home { row.home_short } else { row.away_short },
            logo_url: if home { row.home_logo } else { row.away_logo },
        };
        let goals_for = if home { row.home_score } else { row.away_score };
        let goals_against = if home { row.away_score } else { row.home_score };
        let entry = table.entry(id).or_insert((club, StandingStats::default()));
        entry.1.played += 1;
        entry.1.goals_for += goals_for;
        entry.1.goals_against += goals_against;
        match goals_for.cmp(&goals_against) {
            std::cmp::Ordering::Greater => entry.1.wins += 1,
            std::cmp::Ordering::Less => entry.1.losses += 1,
            std::cmp::Ordering::Equal => entry.1.draws += 1,
        }
    }
    let mut entries: Vec<_> = table.into_iter().collect();
    entries.sort_by(|(_, (club_a, a)), (_, (club_b, b))| {
        b.points()
            .cmp(&a.points())
            .then(b.goal_difference().cmp(&a.goal_difference()))
            .then(b.goals_for.cmp(&a.goals_for))
            .then(club_a.name.cmp(&club_b.name))
    });
    let mut position = 0;
    let mut previous = None;
    entries
        .into_iter()
        .map(|(_, (club, stats))| {
            let signature = (stats.points(), stats.goal_difference(), stats.goals_for);
            if previous != Some(signature) {
                position += 1;
                previous = Some(signature);
            }
            Standing {
                club,
                position,
                played: stats.played,
                wins: stats.wins,
                draws: stats.draws,
                losses: stats.losses,
                goals_for: stats.goals_for,
                goals_against: stats.goals_against,
                goal_difference: stats.goal_difference(),
                points: stats.points(),
            }
        })
        .collect()
}

async fn standings(
    State(app): State<AppState>,
    Query(query): Query<StandingQuery>,
    auth: AuthContext,
) -> ApiResult<Json<Vec<Standing>>> {
    auth.require("read:standings")?;
    let scope = standings_scope(query.scope.as_deref())?;
    let key = format!(
        "api:standings:{}:{}:{}:{}",
        query.season,
        query.group.as_deref().unwrap_or("all"),
        query.competition.as_deref().unwrap_or("all"),
        scope
    );
    if scope != "all" {
        return cached(&app, key, async {
            let mut builder = QueryBuilder::new(
                "SELECT h.id AS home_id, h.canonical_name AS home_name, h.short_name AS home_short, h.logo_url AS home_logo,
                    a.id AS away_id, a.canonical_name AS away_name, a.short_name AS away_short, a.logo_url AS away_logo,
                    m.home_score, m.away_score
                 FROM matches m
                 JOIN clubs h ON h.id = m.home_club_id
                 JOIN clubs a ON a.id = m.away_club_id
                 WHERE m.season_id = ",
            );
            builder.push_bind(&query.season);
            if let Some(group) = &query.group {
                builder.push(" AND m.group_id = ").push_bind(group);
            }
            if let Some(competition) = &query.competition {
                builder.push(" AND EXISTS (SELECT 1 FROM seasons se WHERE se.id = m.season_id AND se.competition_code = ")
                    .push_bind(competition)
                    .push(")");
            }
            builder.push(
                " AND m.home_score IS NOT NULL AND m.away_score IS NOT NULL AND m.status = 'finished'",
            );
            let rows: Vec<StandingSourceRow> = builder.build_query_as().fetch_all(&app.pool).await?;
            Ok(side_standings(rows, scope))
        })
        .await;
    }
    cached(&app, key, async {
        let mut builder = QueryBuilder::new(
            "SELECT c.id AS club_id, c.canonical_name AS club_name, c.short_name AS club_short, c.logo_url AS club_logo,
             s.position, s.played, s.wins, s.draws, s.losses, s.goals_for, s.goals_against, s.goal_difference, s.points
             FROM standings s JOIN clubs c ON c.id = s.club_id WHERE s.season_id = ",
        );
        builder.push_bind(&query.season);
        if let Some(group) = &query.group { builder.push(" AND s.group_id = ").push_bind(group); }
        if let Some(competition) = &query.competition {
            builder.push(" AND EXISTS (SELECT 1 FROM seasons se WHERE se.id = s.season_id AND se.competition_code = ").push_bind(competition).push(")");
        }
        builder.push(" ORDER BY s.group_key, s.position");
        let rows: Vec<StandingRow> = builder.build_query_as().fetch_all(&app.pool).await?;
        Ok(rows.into_iter().map(|r| Standing {
            club: Club { id: r.club_id, name: r.club_name, short_name: r.club_short, logo_url: r.club_logo },
            position: r.position,
            played: r.played,
            wins: r.wins,
            draws: r.draws,
            losses: r.losses,
            goals_for: r.goals_for,
            goals_against: r.goals_against,
            goal_difference: r.goal_difference,
            points: r.points,
        }).collect())
    })
    .await
}

#[derive(Debug, Deserialize)]
struct ClubQuery {
    q: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, FromRow, Serialize, Deserialize)]
struct ClubRow {
    id: Uuid,
    canonical_name: String,
    short_name: String,
    logo_url: Option<String>,
}

async fn clubs(
    State(app): State<AppState>,
    Query(query): Query<ClubQuery>,
    auth: AuthContext,
) -> ApiResult<Json<Vec<Club>>> {
    auth.require("read:clubs")?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let key = format!("api:clubs:{}:{limit}", query.q.as_deref().unwrap_or(""));
    cached(&app, key, async {
        let mut builder = QueryBuilder::new(
            "SELECT id, canonical_name, short_name, logo_url FROM clubs WHERE 1=1",
        );
        if let Some(q) = &query.q {
            builder
                .push(" AND (canonical_name ILIKE ")
                .push_bind(format!("%{q}%"))
                .push(" OR short_name ILIKE ")
                .push_bind(format!("%{q}%"))
                .push(")");
        }
        builder
            .push(" ORDER BY canonical_name LIMIT ")
            .push_bind(limit);
        let rows: Vec<ClubRow> = builder.build_query_as().fetch_all(&app.pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| Club {
                id: r.id,
                name: r.canonical_name,
                short_name: r.short_name,
                logo_url: r.logo_url,
            })
            .collect())
    })
    .await
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
}

async fn search(
    State(app): State<AppState>,
    Query(query): Query<SearchQuery>,
    auth: AuthContext,
) -> ApiResult<Json<Value>> {
    auth.require("read:matches")?;
    let term = query.q.trim();
    if term.is_empty() || term.len() > 100 {
        return Err(ApiError::BadRequest("q must be 1-100 characters".into()));
    }
    let pattern = format!("%{}%", term);
    let club_rows: Vec<ClubRow> = sqlx::query_as(
        "SELECT id, canonical_name, short_name, logo_url FROM clubs
         WHERE canonical_name ILIKE $1 OR short_name ILIKE $2 ORDER BY canonical_name LIMIT 20",
    )
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(&app.pool)
    .await?;
    let match_rows: Vec<MatchRow> = sqlx::query_as(&format!(
        "{MATCH_SELECT} WHERE h.canonical_name ILIKE $1 OR a.canonical_name ILIKE $2
         ORDER BY m.kickoff_utc DESC LIMIT 20"
    ))
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(&app.pool)
    .await?;
    Ok(Json(serde_json::json!({
        "clubs": club_rows.into_iter().map(|r| serde_json::json!({
            "id": r.id, "name": r.canonical_name, "short_name": r.short_name, "logo_url": r.logo_url
        })).collect::<Vec<_>>(),
        "matches": match_rows.into_iter().map(|row| row.into_match(app.config.stale_after_secs)).collect::<Vec<Match>>()
    })))
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
struct StatusRow {
    source: String,
    matches: i64,
    last_match: Option<DateTime<Utc>>,
    live: i64,
    last_polled_at: Option<DateTime<Utc>>,
    stale_matches: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct AccountStatus {
    account_id: uuid::Uuid,
    key_id: uuid::Uuid,
    key_name: String,
    plan: String,
    scopes: Vec<String>,
    quota: i64,
    requests_used: i64,
    requests_remaining: i64,
    rate_limit_per_min: usize,
    rate_remaining: usize,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
struct IngestRunRow {
    mode: String,
    source: String,
    status: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    matches_seen: i64,
    matches_upserted: i64,
    events_upserted: i64,
    standings_rebuilt: i64,
    error: Option<String>,
}

#[derive(Debug, FromRow)]
struct IngestSummaryRow {
    last_success_at: Option<DateTime<Utc>>,
    last_failure_at: Option<DateTime<Utc>>,
    stuck_runs: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct IngestStatus {
    healthy: bool,
    alert_after_secs: u64,
    last_success_at: Option<DateTime<Utc>>,
    last_failure_at: Option<DateTime<Utc>>,
    stuck_runs: i64,
    recent: Vec<IngestRunRow>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StatusResponse {
    account: AccountStatus,
    sources: Vec<StatusRow>,
    ingest: IngestStatus,
}

fn ingest_healthy(summary: &IngestSummaryRow, alert_after_secs: u64) -> bool {
    let Some(success) = summary.last_success_at else {
        return false;
    };
    if summary.stuck_runs > 0 {
        return false;
    }
    if let Some(failure) = summary.last_failure_at {
        if failure > success {
            return false;
        }
    }
    Utc::now().signed_duration_since(success).num_seconds() <= alert_after_secs as i64
}

async fn status(State(app): State<AppState>, auth: AuthContext) -> ApiResult<Json<StatusResponse>> {
    auth.require("read:matches")?;
    let summary: IngestSummaryRow = sqlx::query_as(
        "SELECT MAX(finished_at) FILTER (WHERE status = 'success') AS last_success_at,
                MAX(finished_at) FILTER (WHERE status = 'failed') AS last_failure_at,
                COUNT(*) FILTER (WHERE status = 'running'
                    AND started_at < now() - ($1::bigint * INTERVAL '1 second')) AS stuck_runs
         FROM ingest_runs",
    )
    .bind(app.config.ingest_alert_after_secs as i64)
    .fetch_one(&app.pool)
    .await?;
    let recent: Vec<IngestRunRow> = sqlx::query_as(
        "SELECT mode, source, status, started_at, finished_at, duration_ms,
                matches_seen, matches_upserted, events_upserted, standings_rebuilt, error
         FROM ingest_runs ORDER BY started_at DESC LIMIT 10",
    )
    .fetch_all(&app.pool)
    .await?;
    let healthy = ingest_healthy(&summary, app.config.ingest_alert_after_secs);
    let sources: Vec<StatusRow> = sqlx::query_as(
        "SELECT m.source, COUNT(*) AS matches, MAX(m.kickoff_utc) AS last_match,
                COUNT(*) FILTER (WHERE m.is_live) AS live,
                MAX(m.last_polled_at) AS last_polled_at,
                COUNT(*) FILTER (WHERE m.is_live AND m.last_polled_at < now() - ($1::bigint * INTERVAL '1 second')) AS stale_matches
         FROM matches m GROUP BY m.source ORDER BY m.source",
    )
    .bind(app.config.stale_after_secs as i64)
    .fetch_all(&app.pool)
    .await?;
    let requests_remaining = auth.requests_remaining();
    Ok(Json(StatusResponse {
        account: AccountStatus {
            account_id: auth.account_id,
            key_id: auth.key_id,
            key_name: auth.key_name,
            plan: auth.plan,
            scopes: auth.scopes,
            quota: auth.quota,
            requests_used: auth.requests_used,
            requests_remaining,
            rate_limit_per_min: auth.rate_limit,
            rate_remaining: auth.rate_remaining,
        },
        sources,
        ingest: IngestStatus {
            healthy,
            alert_after_secs: app.config.ingest_alert_after_secs,
            last_success_at: summary.last_success_at,
            last_failure_at: summary.last_failure_at,
            stuck_runs: summary.stuck_runs,
            recent,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn landing_page_links_to_access_request() {
        let Html(html) = landing().await;
        assert!(html.contains("Algerian football data"));
        assert!(html.contains("Request API key"));
        assert!(html.contains("/request-access"));
        assert!(html.contains("/docs"));
    }

    #[tokio::test]
    async fn docs_page_is_readable() {
        let Html(html) = docs().await;
        assert!(html.contains("API v1"));
        assert!(html.contains("Authentication"));
        assert!(html.contains("X-API-Key"));
        assert!(html.contains("/api/v1/matches"));
        assert!(html.contains("X-Requests-Remaining"));
        assert!(html.contains("/openapi.json"));
        assert!(html.contains("/api/v1/teams/{id}/form"));
        assert!(html.contains("/api/v1/teams/{id}/head-to-head"));
        assert!(html.contains("scope=home"));
        assert!(html.contains("ingest.healthy"));
        assert!(html.contains("business"));
        assert!(html.contains("1,000,000"));
    }

    #[tokio::test]
    async fn openapi_documents_freshness_and_headers() {
        let Json(spec) = openapi().await;
        let rendered = spec.to_string();
        assert!(rendered.contains("last_polled_at"));
        assert!(rendered.contains("source_updated_at"));
        assert!(rendered.contains("X-Requests-Remaining"));
        assert!(rendered.contains("StatusResponse"));
        assert!(rendered.contains("TeamForm"));
        assert!(rendered.contains("HeadToHead"));
        assert!(rendered.contains("scope"));
        assert!(rendered.contains("IngestStatus"));
        assert!(rendered.contains("stuck_runs"));
        assert!(rendered.contains("1.4.0"));
    }

    #[test]
    fn cache_control_uses_route_policy() {
        assert_eq!(cache_control("/api/v1/live"), "private, max-age=15");
        assert_eq!(cache_control("/api/v1/matches"), "private, max-age=60");
        assert_eq!(cache_control("/api/v1/status"), "no-store");
    }

    #[test]
    fn request_id_accepts_safe_client_value() {
        let request = Request::builder()
            .header("x-request-id", "client-123")
            .body(Body::empty())
            .unwrap();
        assert_eq!(request_id(&request), "client-123");
    }

    #[test]
    fn request_id_rejects_unsafe_client_value() {
        let request = Request::builder()
            .header("x-request-id", "bad value")
            .body(Body::empty())
            .unwrap();
        let generated = request_id(&request);
        assert_ne!(generated, "bad value");
        assert!(Uuid::parse_str(&generated).is_ok());
    }

    #[test]
    fn status_response_includes_account_usage() {
        let response = StatusResponse {
            account: AccountStatus {
                account_id: Uuid::nil(),
                key_id: Uuid::nil(),
                key_name: "test".into(),
                plan: "free".into(),
                scopes: vec!["read:matches".into()],
                quota: 100,
                requests_used: 12,
                requests_remaining: 88,
                rate_limit_per_min: 300,
                rate_remaining: 288,
            },
            sources: vec![],
            ingest: IngestStatus {
                healthy: true,
                alert_after_secs: 3600,
                last_success_at: None,
                last_failure_at: None,
                stuck_runs: 0,
                recent: vec![],
            },
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["account"]["requests_remaining"], 88);
        assert_eq!(value["account"]["plan"], "free");
        assert_eq!(value["ingest"]["healthy"], true);
    }

    #[test]
    fn ingest_health_requires_recent_success_without_newer_failure() {
        let now = Utc::now();
        let summary = |success, failure, stuck| IngestSummaryRow {
            last_success_at: success,
            last_failure_at: failure,
            stuck_runs: stuck,
        };
        assert!(ingest_healthy(
            &summary(Some(now - chrono::Duration::seconds(60)), None, 0),
            3600
        ));
        assert!(!ingest_healthy(
            &summary(Some(now - chrono::Duration::seconds(4000)), None, 0),
            3600
        ));
        assert!(!ingest_healthy(
            &summary(
                Some(now - chrono::Duration::seconds(120)),
                Some(now - chrono::Duration::seconds(60)),
                0
            ),
            3600
        ));
        assert!(!ingest_healthy(
            &summary(Some(now - chrono::Duration::seconds(60)), None, 1),
            3600
        ));
        assert!(!ingest_healthy(&summary(None, None, 0), 3600));
    }

    #[test]
    fn outcome_classifies_results() {
        assert_eq!(outcome(2, 1), "W");
        assert_eq!(outcome(1, 1), "D");
        assert_eq!(outcome(0, 3), "L");
    }

    #[test]
    fn standings_scope_accepts_only_supported_values() {
        assert_eq!(standings_scope(None).unwrap(), "all");
        assert_eq!(standings_scope(Some("home")).unwrap(), "home");
        assert_eq!(standings_scope(Some("away")).unwrap(), "away");
        assert!(standings_scope(Some("objects")).is_err());
    }

    #[test]
    fn side_standings_split_home_and_away_records() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let row = |home_id, home_score, away_id, away_score| StandingSourceRow {
            home_id,
            home_name: "Home".into(),
            home_short: "H".into(),
            home_logo: None,
            away_id,
            away_name: "Away".into(),
            away_short: "A".into(),
            away_logo: None,
            home_score,
            away_score,
        };
        let rows = vec![row(a, 2, b, 0), row(b, 1, a, 1)];
        let home = side_standings(rows.clone(), "home");
        assert_eq!(home[0].club.id, a);
        assert_eq!(home[0].points, 3);
        assert_eq!(home[1].club.id, b);
        assert_eq!(home[1].points, 1);
        let away = side_standings(rows, "away");
        assert_eq!(away[0].club.id, a);
        assert_eq!(away[0].points, 1);
        assert_eq!(away[1].club.id, b);
        assert_eq!(away[1].points, 0);
    }
}
