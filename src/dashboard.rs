use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::{create_session, delete_session, session_account, SessionAccount};
use crate::crypto::{generate_api_key, hash_password, random_token, verify_password};
use crate::models::ApiError;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/register", get(register_form).post(register))
        .route("/request-access", get(register_form).post(register))
        .route("/login", get(login_form).post(login))
        .route("/logout", post(logout))
        .route("/dashboard", get(dashboard).post(create_key))
        .route("/dashboard/keys/{id}/revoke", post(revoke_key))
}

fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(title: &str, body: String) -> Html<String> {
    Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title><style>
:root{{--ink:#0f172a;--muted:#64748b;--teal:#0f766e;--line:#e2e8f0}}
*{{box-sizing:border-box}}
body{{font-family:Inter,ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif;color:var(--ink);background:radial-gradient(circle at 10% 0%,#ccfbf1 0,transparent 30%),#f8fafc;margin:0;line-height:1.6}}
.wrap{{max-width:960px;margin:auto;padding:0 24px}}
nav{{display:flex;gap:22px;align-items:center;padding:22px 0;font-size:14px}}
a{{color:var(--teal);text-decoration:none;font-weight:600}}
a:hover{{text-decoration:underline}}
h1{{font-size:34px;letter-spacing:-.03em;margin:0 0 16px}}
.card{{background:white;border:1px solid var(--line);border-radius:16px;padding:24px;margin:18px 0;box-shadow:0 4px 16px #0f172a08}}
label{{display:block;font-size:13px;font-weight:700;color:var(--muted);margin-top:12px}}
input,select{{font:inherit;width:100%;padding:11px 12px;border:1px solid #cbd5e1;border-radius:9px;margin:5px 0 10px;background:white;color:var(--ink)}}
button,.btn{{display:inline-flex;align-items:center;justify-content:center;font:inherit;font-weight:700;padding:11px 16px;border:1px solid var(--teal);border-radius:9px;background:var(--teal);color:white;cursor:pointer;text-decoration:none}}
button:hover,.btn:hover{{background:#115e59;text-decoration:none}}
table{{border-collapse:collapse;width:100%}}th,td{{text-align:left;padding:11px 8px;border-bottom:1px solid var(--line);font-size:14px;vertical-align:middle}}
code{{background:#f1f5f9;padding:.15rem .35rem;border-radius:5px}}
pre{{background:#020617;color:#e2e8f0;border-radius:10px;padding:16px;overflow:auto}}
.bad{{color:#b91c1c;font-weight:700}}.ok{{color:#047857;font-weight:700}}
.muted{{color:var(--muted);font-size:14px}}
@media(max-width:640px){{nav{{flex-wrap:wrap;gap:12px}}table{{display:block;overflow:auto}}}}
</style></head><body><div class="wrap"><nav><a href="/">Home</a><a href="/docs">API docs</a><a href="/dashboard">Dashboard</a></nav>{body}</div></body></html>"#,
        title = esc(title)
    ))
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
}

fn csrf_token(headers: &HeaderMap) -> (String, bool) {
    if let Some(value) = cookie_value(headers, "m3_csrf") {
        if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return (value, false);
        }
    }
    (random_token(32), true)
}

fn verify_csrf(headers: &HeaderMap, provided: Option<String>) -> Result<(), ApiError> {
    let expected = cookie_value(headers, "m3_csrf")
        .ok_or_else(|| ApiError::BadRequest("invalid csrf token".into()))?;
    match provided {
        Some(value) if value.len() == 64 && value == expected => Ok(()),
        _ => Err(ApiError::BadRequest("invalid csrf token".into())),
    }
}

fn csrf_input(token: &str) -> String {
    format!("<input type='hidden' name='csrf' value='{}'>", esc(token))
}

fn cookie_header(
    name: &str,
    value: &str,
    max_age: i64,
    secure: bool,
) -> Result<HeaderValue, ApiError> {
    let suffix = if secure { "; Secure" } else { "" };
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{suffix}")
        .parse()
        .map_err(|_| ApiError::Internal("failed to build cookie".into()))
}

fn page_response(
    title: &str,
    body: String,
    csrf: &str,
    set_csrf: bool,
    secure: bool,
    ttl_seconds: i64,
) -> Result<Response, ApiError> {
    let mut response = page(title, body).into_response();
    if set_csrf {
        response.headers_mut().append(
            header::SET_COOKIE,
            cookie_header("m3_csrf", csrf, ttl_seconds, secure)?,
        );
    }
    Ok(response)
}

fn redirect_with_cookie(
    app: &AppState,
    location: &str,
    token: &str,
    csrf: &str,
) -> Result<Response, ApiError> {
    let ttl = app.config.session_ttl_hours.saturating_mul(3600);
    let mut response = (StatusCode::SEE_OTHER, HeaderMap::new()).into_response();
    response.headers_mut().insert(
        header::LOCATION,
        location
            .parse()
            .map_err(|_| ApiError::Internal("invalid redirect".into()))?,
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header("m3_session", token, ttl, app.config.cookie_secure)?,
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header("m3_csrf", csrf, ttl, app.config.cookie_secure)?,
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct RegisterForm {
    email: String,
    name: String,
    password: String,
    csrf: Option<String>,
}

async fn register_form(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (csrf, set_csrf) = csrf_token(&headers);
    if !app.config.registrations_enabled {
        let body = r#"<div class="card"><h1>Registration closed</h1><p>Contact the administrator for an account.</p></div>"#.to_string();
        return page_response(
            "Register",
            body,
            &csrf,
            set_csrf,
            app.config.cookie_secure,
            app.config.session_ttl_hours * 3600,
        );
    }
    let hidden = csrf_input(&csrf);
    let body = format!(
        r#"<div class="card"><h1>Request API access</h1><p class="muted">Create an account, then generate your key instantly from the dashboard.</p>
 <form method="post" action="/register">
 {hidden}
 <label for="name">Name</label><input id="name" name="name" placeholder="Your name" required>
 <label for="email">Email</label><input id="email" name="email" type="email" placeholder="you@example.com" required>
 <label for="password">Password</label><input id="password" name="password" type="password" placeholder="At least 12 characters" minlength="12" required>
 <button type="submit">Create account</button></form>
 <p class="muted">Already have an account? <a href="/login">Log in</a>.</p></div>"#
    );
    page_response(
        "Register",
        body,
        &csrf,
        set_csrf,
        app.config.cookie_secure,
        app.config.session_ttl_hours * 3600,
    )
}

async fn register(
    State(app): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RegisterForm>,
) -> Result<Response, ApiError> {
    verify_csrf(&headers, form.csrf)?;
    if !app.config.registrations_enabled {
        return Err(ApiError::Forbidden("registration is closed".into()));
    }
    let email = form.email.trim().to_lowercase();
    let name = form.name.trim();
    if !email.contains('@') || name.is_empty() || form.password.len() < 12 {
        return Err(ApiError::BadRequest(
            "valid email, name and 12+ character password required".into(),
        ));
    }
    let hash =
        hash_password(&form.password).map_err(|_| ApiError::Internal("hash failed".into()))?;
    let free_quota: i64 = sqlx::query_scalar("SELECT monthly_quota FROM plans WHERE name = 'free'")
        .fetch_one(&app.pool)
        .await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO accounts (email, name, password_hash, quota) VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(email)
    .bind(name)
    .bind(hash)
    .bind(free_quota)
    .fetch_one(&app.pool)
    .await
    .map_err(|err| {
        if err.to_string().contains("duplicate") {
            ApiError::BadRequest("email already registered".into())
        } else {
            ApiError::from(err)
        }
    })?;
    let token = create_session(&app, id).await?;
    let (csrf, _) = csrf_token(&headers);
    redirect_with_cookie(&app, "/dashboard", &token, &csrf)
}

#[derive(Debug, Deserialize)]
struct LoginForm {
    email: String,
    password: String,
    csrf: Option<String>,
}

async fn login_form(headers: HeaderMap, State(app): State<AppState>) -> Result<Response, ApiError> {
    let (csrf, set_csrf) = csrf_token(&headers);
    let hidden = csrf_input(&csrf);
    let body = format!(
        r#"<div class="card"><h1>Welcome back</h1><p class="muted">Log in to manage your API keys and usage.</p>
 <form method="post" action="/login">
 {hidden}
 <label for="email">Email</label><input id="email" name="email" type="email" placeholder="you@example.com" required>
 <label for="password">Password</label><input id="password" name="password" type="password" placeholder="Your password" required>
 <button type="submit">Log in</button></form>
 <p class="muted">No account? <a href="/request-access">Request API access</a>.</p></div>"#
    );
    page_response(
        "Log in",
        body,
        &csrf,
        set_csrf,
        app.config.cookie_secure,
        app.config.session_ttl_hours * 3600,
    )
}

async fn login(
    State(app): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<Response, ApiError> {
    verify_csrf(&headers, form.csrf)?;
    let email = form.email.trim().to_lowercase();
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, password_hash FROM accounts WHERE email = $1")
            .bind(&email)
            .fetch_optional(&app.pool)
            .await?;
    let Some((account_id, hash)) = row else {
        let dummy = hash_password("account-does-not-exist")?;
        let _ = verify_password(&form.password, &dummy);
        return Err(ApiError::Unauthorized("invalid email or password".into()));
    };
    if !verify_password(&form.password, &hash) {
        return Err(ApiError::Unauthorized("invalid email or password".into()));
    }
    let token = create_session(&app, account_id).await?;
    let (csrf, _) = csrf_token(&headers);
    redirect_with_cookie(&app, "/dashboard", &token, &csrf)
}

async fn logout(State(app): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    delete_session(&app, &headers).await?;
    let mut response = Redirect::to("/login").into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header("m3_session", "", 0, app.config.cookie_secure)?,
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie_header("m3_csrf", "", 0, app.config.cookie_secure)?,
    );
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct CreateKeyForm {
    name: String,
    scopes: Option<String>,
    csrf: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RevokeForm {
    csrf: Option<String>,
}

async fn dashboard(State(app): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let (csrf, set_csrf) = csrf_token(&headers);
    let Some(account) = session_account(&app, &headers).await else {
        let body = "<div class='card'><p>Please <a href='/login'>log in</a>.</p></div>".to_string();
        return page_response(
            "Dashboard",
            body,
            &csrf,
            set_csrf,
            app.config.cookie_secure,
            app.config.session_ttl_hours * 3600,
        );
    };
    let Html(body) = render_dashboard(&app, &account, None, &csrf).await?;
    page_response(
        "Dashboard",
        body,
        &csrf,
        set_csrf,
        app.config.cookie_secure,
        app.config.session_ttl_hours * 3600,
    )
}

async fn create_key(
    State(app): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CreateKeyForm>,
) -> Result<Response, ApiError> {
    verify_csrf(&headers, form.csrf)?;
    let Some(account) = session_account(&app, &headers).await else {
        return Err(ApiError::Unauthorized("login required".into()));
    };
    let name = form.name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(ApiError::BadRequest(
            "key name must be 1-80 characters".into(),
        ));
    }
    let scopes: Vec<String> = form
        .scopes
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let allowed = ["read:matches", "read:standings", "read:clubs"];
    let scopes: Vec<String> = scopes
        .into_iter()
        .filter(|scope| allowed.contains(&scope.as_str()))
        .collect();
    let scopes = if scopes.is_empty() {
        vec![
            "read:matches".into(),
            "read:standings".into(),
            "read:clubs".into(),
        ]
    } else {
        scopes
    };
    let generated = generate_api_key();
    sqlx::query(
        "INSERT INTO api_keys (account_id, name, key_prefix, key_hash, scopes, quota)
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(account.id)
    .bind(name)
    .bind(&generated.prefix)
    .bind(&generated.hash)
    .bind(&scopes)
    .bind(account.quota)
    .execute(&app.pool)
    .await?;
    let (csrf, set_csrf) = csrf_token(&headers);
    let Html(body) = render_dashboard(&app, &account, Some(&generated.raw), &csrf).await?;
    page_response(
        "Dashboard",
        body,
        &csrf,
        set_csrf,
        app.config.cookie_secure,
        app.config.session_ttl_hours * 3600,
    )
}

async fn revoke_key(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Form(form): Form<RevokeForm>,
) -> Result<Redirect, ApiError> {
    verify_csrf(&headers, form.csrf)?;
    let Some(account) = session_account(&app, &headers).await else {
        return Err(ApiError::Unauthorized("login required".into()));
    };
    sqlx::query("UPDATE api_keys SET revoked = TRUE WHERE id = $1 AND account_id = $2")
        .bind(id)
        .bind(account.id)
        .execute(&app.pool)
        .await?;
    Ok(Redirect::to("/dashboard"))
}

#[derive(Debug, FromRow)]
struct KeyRow {
    id: Uuid,
    name: String,
    key_prefix: String,
    scopes: Vec<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    revoked: bool,
}

async fn render_dashboard(
    app: &AppState,
    account: &SessionAccount,
    new_key: Option<&str>,
    csrf: &str,
) -> Result<Html<String>, ApiError> {
    let keys: Vec<KeyRow> = sqlx::query_as(
        "SELECT id, name, key_prefix, scopes, created_at, last_used_at, revoked
         FROM api_keys WHERE account_id = $1 ORDER BY created_at DESC",
    )
    .bind(account.id)
    .fetch_all(&app.pool)
    .await?;
    let used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(u.requests),0)::bigint FROM usage_daily u
         JOIN api_keys k ON k.id = u.key_id
         WHERE k.account_id = $1 AND u.day >= DATE_TRUNC('month', now())::date",
    )
    .bind(account.id)
    .fetch_one(&app.pool)
    .await?;

    let banner = new_key
        .map(|key| format!(
            r#"<div class="card"><h3 class="ok">API key created</h3><p>Copy it now; it will not be shown again:</p><pre><code>{}</code></pre></div>"#,
            esc(key)
        ))
        .unwrap_or_default();
    let hidden = csrf_input(csrf);

    let rows = keys
        .iter()
        .map(|row| {
            let status = if row.revoked { "<span class='bad'>revoked</span>" } else { "<span class='ok'>active</span>" };
            format!(
                "<tr><td>{}</td><td><code>m3_{}…</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><form method='post' action='/dashboard/keys/{}/revoke'>{}<button type='submit'>Revoke</button></form></td></tr>",
                esc(&row.name), esc(&row.key_prefix), status, esc(&row.scopes.join(", ")),
                row.created_at.format("%Y-%m-%d"), row.last_used_at.map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "never".into()), row.id, hidden
            )
        })
        .collect::<Vec<_>>()
        .join("");

    let body = format!(
        r#"<div class="card"><h1>Dashboard</h1><p><strong>{}</strong> ({}) · plan: {} · monthly quota: {} · used this month: {}</p>
<form method="post" action="/logout">{hidden}<button type="submit">Log out</button></form></div>
{}
<div class="card"><h2>Create API key</h2><p class="muted">Name your key, choose its scopes, and copy it once after creation.</p>
 <form method="post" action="/dashboard">
 {hidden}
 <label for="key-name">Key name</label><input id="key-name" name="name" placeholder="Production app" required>
 <label for="key-scopes">Scopes</label><select id="key-scopes" name="scopes"><option value="read:matches,read:standings,read:clubs">All read scopes</option><option value="read:matches,read:standings">Matches + standings</option><option value="read:matches">Matches only</option></select>
 <button type="submit">Create key</button></form></div>
<div class="card"><h2>Your keys</h2><table><thead><tr><th>Name</th><th>Key</th><th>Status</th><th>Scopes</th><th>Created</th><th>Last used</th><th></th></tr></thead><tbody>{}</tbody></table></div>"#,
        esc(&account.name),
        esc(&account.email),
        esc(&account.plan),
        account.quota,
        used,
        banner,
        rows
    );
    Ok(page("Dashboard", body))
}
