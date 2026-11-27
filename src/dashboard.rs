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
        .route("/billing", get(billing_form).post(submit_payment_request))
        .route("/admin", get(admin_dashboard))
        .route("/admin/plans", post(assign_plan))
        .route("/admin/payments/{id}/approve", post(approve_payment))
        .route("/admin/payments/{id}/reject", post(reject_payment))
        .route("/admin/payment-settings", post(update_payment_settings))
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

#[derive(Debug, Deserialize)]
struct PaymentRequestForm {
    plan: String,
    amount: String,
    tx_hash: String,
    note: Option<String>,
    csrf: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AssignPlanForm {
    email: String,
    plan: String,
    csrf: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PaymentSettingsForm {
    currency: String,
    network: String,
    wallet_address: String,
    instructions: String,
    csrf: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewPaymentForm {
    review_note: Option<String>,
    csrf: Option<String>,
}

async fn current_user(app: &AppState, headers: &HeaderMap) -> Result<SessionAccount, ApiError> {
    session_account(app, headers)
        .await
        .ok_or_else(|| ApiError::Unauthorized("login required".into()))
}

async fn current_admin(app: &AppState, headers: &HeaderMap) -> Result<SessionAccount, ApiError> {
    let account = current_user(app, headers).await?;
    account.require_admin()?;
    Ok(account)
}

#[derive(Debug, FromRow)]
struct PaymentSettings {
    currency: String,
    network: String,
    wallet_address: String,
    instructions: String,
}

#[derive(Debug, FromRow)]
struct PaymentRequestRow {
    id: Uuid,
    email: String,
    name: String,
    plan: String,
    amount_micros: i64,
    currency: String,
    network: String,
    tx_hash: String,
    note: String,
    status: String,
    review_note: String,
    created_at: chrono::DateTime<chrono::Utc>,
    reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, FromRow)]
struct AdminUserRow {
    email: String,
    name: String,
    plan: String,
    quota: i64,
    is_admin: bool,
    used: i64,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn amount_text(micros: i64) -> String {
    format!("{:.6}", micros as f64 / 1_000_000.0)
}

fn status_badge(status: &str) -> &'static str {
    match status {
        "approved" => "<span class='ok'>approved</span>",
        "rejected" => "<span class='bad'>rejected</span>",
        _ => "<span>pending</span>",
    }
}

async fn payment_settings(app: &AppState) -> Result<PaymentSettings, ApiError> {
    sqlx::query_as(
        "SELECT currency, network, wallet_address, instructions FROM payment_settings WHERE enabled = TRUE",
    )
    .fetch_one(&app.pool)
    .await
    .map_err(Into::into)
}

async fn plan_options(app: &AppState, selected: &str) -> Result<String, ApiError> {
    let plans: Vec<(String, i64)> =
        sqlx::query_as("SELECT name, monthly_quota FROM plans ORDER BY monthly_quota")
            .fetch_all(&app.pool)
            .await?;
    Ok(plans
        .into_iter()
        .map(|(name, quota)| {
            let mark = if name == selected { " selected" } else { "" };
            format!(
                "<option value='{}'{}>{} — {} requests/month</option>",
                esc(&name),
                mark,
                esc(&name),
                quota
            )
        })
        .collect::<Vec<_>>()
        .join(""))
}

async fn billing_form(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (csrf, set_csrf) = csrf_token(&headers);
    let Some(account) = session_account(&app, &headers).await else {
        let body = "<div class='card'><p>Please <a href='/login'>log in</a>.</p></div>".to_string();
        return page_response(
            "Billing",
            body,
            &csrf,
            set_csrf,
            app.config.cookie_secure,
            app.config.session_ttl_hours * 3600,
        );
    };
    let settings = payment_settings(&app).await?;
    let options = plan_options(&app, &account.plan).await?;
    let requests: Vec<PaymentRequestRow> = sqlx::query_as(
        "SELECT p.id, a.email, a.name, p.plan, p.amount_micros, p.currency, p.network,
                p.tx_hash, p.note, p.status, p.review_note, p.created_at, p.reviewed_at
         FROM payment_requests p JOIN accounts a ON a.id = p.account_id
         WHERE p.account_id = $1 ORDER BY p.created_at DESC LIMIT 50",
    )
    .bind(account.id)
    .fetch_all(&app.pool)
    .await?;
    let hidden = csrf_input(&csrf);
    let wallet = if settings.wallet_address.is_empty() {
        "not configured yet".to_string()
    } else {
        format!("<code>{}</code>", esc(&settings.wallet_address))
    };
    let rows = requests
        .iter()
        .map(|row| {
            format!(
                "<tr><td>{}</td><td>{} {}</td><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td></tr>",
                row.created_at.format("%Y-%m-%d %H:%M"),
                amount_text(row.amount_micros), esc(&row.currency), esc(&row.plan),
                esc(&row.network), esc(&row.tx_hash), status_badge(&row.status),
                esc(&row.review_note),
                row.reviewed_at.map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let body = format!(
        r#"<div class="card"><h1>Billing</h1><p><strong>{}</strong> · current plan: {}</p>
 <p class="muted">Free beta: submit a USDT payment reference and an administrator will apply the plan after review.</p>
 <form method="post" action="/billing">
 {hidden}
 <label for="billing-plan">Plan</label><select id="billing-plan" name="plan">{options}</select>
 <label for="billing-amount">Amount ({currency})</label><input id="billing-amount" name="amount" type="number" min="0" step="0.000001" required>
 <label for="billing-tx">Transaction hash</label><input id="billing-tx" name="tx_hash" placeholder="Paste the transaction hash" maxlength="200" required>
 <label for="billing-note">Note</label><input id="billing-note" name="note" placeholder="Optional details" maxlength="500">
 <button type="submit">Submit payment</button></form></div>
 <div class="card"><h2>Payment method</h2><p>Currency: <code>{currency}</code> · Network: <code>{network}</code></p><p>Wallet: {wallet}</p><pre>{instructions}</pre></div>
  <div class="card"><h2>Your payment requests</h2><table><thead><tr><th>Submitted</th><th>Amount</th><th>Plan</th><th>Network</th><th>Transaction</th><th>Status</th><th>Review note</th><th>Reviewed</th></tr></thead><tbody>{rows}</tbody></table></div>"#,
        esc(&account.name),
        esc(&account.plan),
        hidden = hidden,
        options = options,
        currency = esc(&settings.currency),
        network = esc(&settings.network),
        wallet = wallet,
        instructions = esc(&settings.instructions),
        rows = rows
    );
    page_response(
        "Billing",
        body,
        &csrf,
        set_csrf,
        app.config.cookie_secure,
        app.config.session_ttl_hours * 3600,
    )
}

async fn submit_payment_request(
    State(app): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<PaymentRequestForm>,
) -> Result<Redirect, ApiError> {
    verify_csrf(&headers, form.csrf)?;
    let account = current_user(&app, &headers).await?;
    let plan = form.plan.trim().to_lowercase();
    let exists: Option<i64> = sqlx::query_scalar("SELECT monthly_quota FROM plans WHERE name = $1")
        .bind(&plan)
        .fetch_optional(&app.pool)
        .await?;
    if exists.is_none() {
        return Err(ApiError::BadRequest("unknown plan".into()));
    }
    let amount: f64 = form
        .amount
        .trim()
        .parse()
        .map_err(|_| ApiError::BadRequest("invalid amount".into()))?;
    if !amount.is_finite() || amount < 0.0 || amount > 1_000_000_000.0 {
        return Err(ApiError::BadRequest("amount out of range".into()));
    }
    let amount_micros = (amount * 1_000_000.0).round() as i64;
    let tx_hash = form.tx_hash.trim();
    if tx_hash.is_empty() || tx_hash.len() > 200 {
        return Err(ApiError::BadRequest(
            "transaction hash must be 1-200 characters".into(),
        ));
    }
    let note = form.note.unwrap_or_default().trim().to_string();
    if note.len() > 500 {
        return Err(ApiError::BadRequest(
            "note must be at most 500 characters".into(),
        ));
    }
    let settings = payment_settings(&app).await?;
    sqlx::query(
        "INSERT INTO payment_requests
            (id, account_id, plan, amount_micros, currency, network, wallet_address, tx_hash, note)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(Uuid::new_v4())
    .bind(account.id)
    .bind(&plan)
    .bind(amount_micros)
    .bind(&settings.currency)
    .bind(&settings.network)
    .bind(&settings.wallet_address)
    .bind(tx_hash)
    .bind(note)
    .execute(&app.pool)
    .await?;
    Ok(Redirect::to("/billing"))
}

async fn admin_dashboard(
    State(app): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (csrf, set_csrf) = csrf_token(&headers);
    let Some(account) = session_account(&app, &headers).await else {
        let body = "<div class='card'><p>Please <a href='/login'>log in</a>.</p></div>".to_string();
        return page_response(
            "Admin",
            body,
            &csrf,
            set_csrf,
            app.config.cookie_secure,
            app.config.session_ttl_hours * 3600,
        );
    };
    if !account.is_admin {
        let body =
            "<div class='card'><h1>Forbidden</h1><p>Admin access required.</p></div>".to_string();
        return page_response(
            "Admin",
            body,
            &csrf,
            set_csrf,
            app.config.cookie_secure,
            app.config.session_ttl_hours * 3600,
        );
    }
    let users: Vec<AdminUserRow> = sqlx::query_as(
        "SELECT a.email, a.name, a.plan, a.quota, a.is_admin,
                COALESCE(SUM(u.requests), 0)::bigint AS used, a.created_at
         FROM accounts a
         LEFT JOIN api_keys k ON k.account_id = a.id
         LEFT JOIN usage_daily u ON u.key_id = k.id AND u.day >= DATE_TRUNC('month', now())::date
         GROUP BY a.id ORDER BY a.created_at DESC LIMIT 100",
    )
    .fetch_all(&app.pool)
    .await?;
    let requests: Vec<PaymentRequestRow> = sqlx::query_as(
        "SELECT p.id, a.email, a.name, p.plan, p.amount_micros, p.currency, p.network,
                p.tx_hash, p.note, p.status, p.review_note, p.created_at, p.reviewed_at
         FROM payment_requests p JOIN accounts a ON a.id = p.account_id
         ORDER BY p.created_at DESC LIMIT 100",
    )
    .fetch_all(&app.pool)
    .await?;
    let settings = payment_settings(&app).await?;
    let options = plan_options(&app, "free").await?;
    let hidden = csrf_input(&csrf);
    let user_rows = users
        .iter()
        .map(|row| {
            let admin = if row.is_admin { "<span class='ok'>admin</span>" } else { "user" };
            format!(
                "<tr><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&row.email), esc(&row.name), esc(&row.plan), admin,
                row.quota, row.used, row.created_at.format("%Y-%m-%d")
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let payment_rows = requests
        .iter()
        .map(|row| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{} {}</td><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td><form method='post' action='/admin/payments/{}/approve'>{}<input name='review_note' placeholder='Review note' maxlength='500'><button type='submit'>Approve</button></form><form method='post' action='/admin/payments/{}/reject'>{}<input name='review_note' placeholder='Reason' maxlength='500'><button type='submit'>Reject</button></form></td></tr>",
                esc(&row.email), esc(&row.name), esc(&row.plan), amount_text(row.amount_micros),
                esc(&row.currency), esc(&row.network), esc(&row.tx_hash), esc(&row.note),
                status_badge(&row.status), row.id, hidden, row.id, hidden
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let body = format!(
        r#"<div class="card"><h1>Admin</h1><p>Signed in as <strong>{}</strong> · <a href="/dashboard">User dashboard</a></p>
 <form method="post" action="/logout">{hidden}<button type="submit">Log out</button></form></div>
 <div class="card"><h2>Accounts</h2><table><thead><tr><th>Email</th><th>Name</th><th>Plan</th><th>Role</th><th>Quota</th><th>Used</th><th>Created</th></tr></thead><tbody>{user_rows}</tbody></table></div>
 <div class="card"><h2>Assign plan</h2><form method="post" action="/admin/plans">{hidden}
 <label for="assign-email">Account email</label><input id="assign-email" name="email" type="email" required>
 <label for="assign-plan">Plan</label><select id="assign-plan" name="plan">{options}</select>
 <button type="submit">Assign plan</button></form></div>
  <div class="card"><h2>Payment requests</h2><table><thead><tr><th>Account</th><th>Name</th><th>Plan</th><th>Amount</th><th>Network</th><th>Transaction</th><th>Note</th><th>Status</th><th>Review</th></tr></thead><tbody>{payment_rows}</tbody></table></div>

 <div class="card"><h2>USDT payment method</h2><form method="post" action="/admin/payment-settings">{hidden}
 <label for="pay-currency">Currency</label><input id="pay-currency" name="currency" value="{currency}" required>
 <label for="pay-network">Network</label><input id="pay-network" name="network" value="{network}" placeholder="TRC-20" maxlength="80">
 <label for="pay-wallet">Wallet address</label><input id="pay-wallet" name="wallet_address" value="{wallet}" maxlength="200">
 <label for="pay-instructions">Instructions</label><input id="pay-instructions" name="instructions" value="{instructions}" maxlength="1000">
 <button type="submit">Save payment method</button></form></div>"#,
        esc(&account.email),
        hidden = hidden,
        user_rows = user_rows,
        options = options,
        payment_rows = payment_rows,
        currency = esc(&settings.currency),
        network = esc(&settings.network),
        wallet = esc(&settings.wallet_address),
        instructions = esc(&settings.instructions)
    );
    page_response(
        "Admin",
        body,
        &csrf,
        set_csrf,
        app.config.cookie_secure,
        app.config.session_ttl_hours * 3600,
    )
}

async fn assign_plan(
    State(app): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AssignPlanForm>,
) -> Result<Redirect, ApiError> {
    verify_csrf(&headers, form.csrf)?;
    current_admin(&app, &headers).await?;
    let email = form.email.trim().to_lowercase();
    let plan = form.plan.trim().to_lowercase();
    let mut tx = app.pool.begin().await?;
    let quota: Option<i64> = sqlx::query_scalar("SELECT monthly_quota FROM plans WHERE name = $1")
        .bind(&plan)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(quota) = quota else {
        return Err(ApiError::BadRequest("unknown plan".into()));
    };
    let account_id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE accounts SET plan = $2, quota = $3 WHERE email = $1 RETURNING id",
    )
    .bind(email)
    .bind(&plan)
    .bind(quota)
    .fetch_optional(&mut *tx)
    .await?;
    if account_id.is_none() {
        return Err(ApiError::BadRequest("account not found".into()));
    }
    sqlx::query("UPDATE api_keys SET quota = $2 WHERE account_id = $1")
        .bind(account_id.unwrap())
        .bind(quota)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Redirect::to("/admin"))
}

async fn approve_payment(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Form(form): Form<ReviewPaymentForm>,
) -> Result<Redirect, ApiError> {
    review_payment(&app, &headers, id, form, true).await
}

async fn reject_payment(
    State(app): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Form(form): Form<ReviewPaymentForm>,
) -> Result<Redirect, ApiError> {
    review_payment(&app, &headers, id, form, false).await
}

async fn review_payment(
    app: &AppState,
    headers: &HeaderMap,
    id: Uuid,
    form: ReviewPaymentForm,
    approve: bool,
) -> Result<Redirect, ApiError> {
    verify_csrf(headers, form.csrf)?;
    let admin = current_admin(app, headers).await?;
    let review_note = form.review_note.unwrap_or_default().trim().to_string();
    if review_note.len() > 500 {
        return Err(ApiError::BadRequest(
            "review note must be at most 500 characters".into(),
        ));
    }
    let mut tx = app.pool.begin().await?;
    let request: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT account_id, plan FROM payment_requests WHERE id = $1 AND status = 'pending'",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((account_id, plan)) = request else {
        return Err(ApiError::BadRequest(
            "payment request is not pending".into(),
        ));
    };
    if approve {
        let quota: i64 = sqlx::query_scalar("SELECT monthly_quota FROM plans WHERE name = $1")
            .bind(&plan)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("UPDATE accounts SET plan = $2, quota = $3 WHERE id = $1")
            .bind(account_id)
            .bind(&plan)
            .bind(quota)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE api_keys SET quota = $2 WHERE account_id = $1")
            .bind(account_id)
            .bind(quota)
            .execute(&mut *tx)
            .await?;
    }
    let status = if approve { "approved" } else { "rejected" };
    sqlx::query(
        "UPDATE payment_requests SET status = $2, reviewed_at = now(), reviewed_by = $3, review_note = $4
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(admin.id)
    .bind(review_note)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Redirect::to("/admin"))
}

async fn update_payment_settings(
    State(app): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<PaymentSettingsForm>,
) -> Result<Redirect, ApiError> {
    verify_csrf(&headers, form.csrf)?;
    current_admin(&app, &headers).await?;
    let currency = form.currency.trim().to_uppercase();
    let network = form.network.trim().to_string();
    let wallet_address = form.wallet_address.trim().to_string();
    let instructions = form.instructions.trim().to_string();
    if currency.is_empty() || currency.len() > 10 {
        return Err(ApiError::BadRequest(
            "currency must be 1-10 characters".into(),
        ));
    }
    if network.len() > 80 || wallet_address.len() > 200 || instructions.len() > 1000 {
        return Err(ApiError::BadRequest(
            "payment method fields are too long".into(),
        ));
    }
    sqlx::query(
        "UPDATE payment_settings SET currency = $1, network = $2, wallet_address = $3,
            instructions = $4, updated_at = now() WHERE enabled = TRUE",
    )
    .bind(currency)
    .bind(network)
    .bind(wallet_address)
    .bind(instructions)
    .execute(&app.pool)
    .await?;
    Ok(Redirect::to("/admin"))
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

    let admin_link = if account.is_admin {
        " · <a href='/admin'>Admin</a>"
    } else {
        ""
    };
    let body = format!(
        r#"<div class="card"><h1>Dashboard</h1><p><strong>{}</strong> ({}) · plan: {} · monthly quota: {} · used this month: {}</p>
<p><a href="/billing">Billing</a>{admin_link}</p>
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
