use actix_web::{dev::Payload, Error, FromRequest, HttpRequest};
use actix_web::{middleware::Logger, web, App, HttpResponse, HttpServer, Responder};
use chrono::{DateTime, NaiveDateTime, Utc};
use futures::future::{ready, Ready};
use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row, ValueRef};
use std::collections::HashSet;
use std::env;

use once_cell::sync::Lazy;

struct UntypedString(String);

impl sqlx::Type<sqlx::Postgres> for UntypedString {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl<'q> sqlx::Encode<'q, sqlx::Postgres> for UntypedString {
    fn encode_by_ref(&self, buf: &mut sqlx::postgres::PgArgumentBuffer) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>> {
        <String as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }
}

// Global set of valid tokens, initialized once from AUTH_TOKENS env var.
static VALID_TOKENS: Lazy<HashSet<String>> = Lazy::new(|| {
    let tokens_str = env::var("AUTH_TOKENS").unwrap_or_else(|_| {
        log::warn!("AUTH_TOKENS environment variable not set. Authentication will be disabled.");
        "".to_string()
    });

    tokens_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
});

// --- Bearer Token Authentication Request Guard ---

pub struct BearerAuth;

impl FromRequest for BearerAuth {
    type Error = Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let auth_header = req.headers().get("Authorization");

        let result = match auth_header {
            Some(header_value) => {
                if let Ok(header_str) = header_value.to_str() {
                    if header_str.starts_with("Bearer ") {
                        let token = &header_str[7..];
                        if VALID_TOKENS.contains(token) {
                            Ok(BearerAuth)
                        } else {
                            log::debug!("Invalid Bearer token provided.");
                            Err(actix_web::error::ErrorUnauthorized("Invalid token"))
                        }
                    } else {
                        log::debug!("Authorization header present but not Bearer scheme.");
                        Err(actix_web::error::ErrorUnauthorized(
                            "Invalid authorization scheme",
                        ))
                    }
                } else {
                    log::debug!("Authorization header contains non-string data.");
                    Err(actix_web::error::ErrorBadRequest("Invalid header encoding"))
                }
            }
            None => {
                log::debug!("Authorization header missing.");
                Err(actix_web::error::ErrorUnauthorized(
                    "Authorization header missing",
                ))
            }
        };

        ready(result)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
struct ProxyRequest {
    sql: String,
    params: Option<Vec<Value>>,
    method: String, // "run" | "all" | "values" | "get" | "execute"
}

#[derive(Debug, Serialize)]
struct Rows2d {
    rows: Vec<Vec<Value>>,
}

#[derive(Debug, Serialize)]
struct Row1d {
    rows: Vec<Value>,
}

async fn execute_handler(
    _auth: BearerAuth,
    db: web::Data<PgPool>,
    body: web::Json<ProxyRequest>,
) -> impl Responder {
    let req = body.into_inner();

    // Basic safety guard: disallow empty SQL.
    if req.sql.trim().is_empty() {
        return HttpResponse::BadRequest().body("sql must not be empty");
    }

    // Create query and bind params sequentially.
    let mut q = sqlx::query(&req.sql);
    if let Some(params) = req.params {
        for p in params {
            // bind param as a string representation to keep things simple.
            // Advanced: you'd detect types and bind accordingly.
            q = match p {
                serde_json::Value::String(s) => {
                    // Attempt to parse string as NaiveDateTime (timestamp without time zone)
                    // This handles common formats for timestamps.
                    let naive_dt = NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S")
                        .or_else(|_| NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f"))
                        .or_else(|_| NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S"))
                        .or_else(|_| NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.f"));

                    if let Ok(dt) = naive_dt {
                        q.bind(dt)
                    } else if let Ok(dt_utc) = s.parse::<DateTime<Utc>>() {
                        // If it's a timestamp with timezone (like ISO 8601 Z), convert to NaiveDateTime
                        // Note: This assumes the user wants to store the UTC time without timezone info.
                        q.bind(dt_utc.naive_utc())
                    } else {
                        // Fallback to binding as String (TEXT)
                        q.bind(UntypedString(s))
                    }
                }
                serde_json::Value::Number(n) => {
                    if n.is_i64() {
                        // Bind integers (like LIMIT/OFFSET values) as i64 (BIGINT)
                        q.bind(n.as_i64().unwrap())
                    } else {
                        // Bind other numbers (floats) as strings
                        q.bind(n.to_string())
                    }
                }
                serde_json::Value::Bool(b) => q.bind(b),
                _ => q.bind(UntypedString(p.to_string())), // Fallback for other types
            };
        }
    }

    // Limit number of returned rows for safety (example).
    // You can remove or tune as you need; it's a good safety measure.
    // We'll not enforce here hard limit in SQL — caller controls query — so return size-check after fetch.
    match req.method.as_str() {
        "get" => match q.fetch_one(db.get_ref()).await {
            Ok(row) => {
                let mut out: Vec<Value> = Vec::with_capacity(row.len());
                for (i, _) in row.columns().iter().enumerate() {
                    match row_to_value(&row, i) {
                        Ok(v) => out.push(v),
                        Err(e) => {
                            warn!("column conversion error: {:?}", e);
                            out.push(Value::String("<<conversion error>>".to_string()));
                        }
                    }
                }
                HttpResponse::Ok().json(Row1d { rows: out })
            }
            Err(e) => HttpResponse::InternalServerError().body(format!("DB error: {}", e)),
        },
        "all" | "values" => match q.fetch_all(db.get_ref()).await {
            Ok(rows) => {
                let mut out: Vec<Vec<Value>> = Vec::with_capacity(rows.len());
                for row in rows {
                    let mut r: Vec<Value> = Vec::with_capacity(row.len());
                    for (i, _) in row.columns().iter().enumerate() {
                        match row_to_value(&row, i) {
                            Ok(v) => r.push(v),
                            Err(e) => {
                                warn!("column conversion error: {:?}", e);
                                r.push(Value::String("<<conversion error>>".to_string()));
                            }
                        }
                    }
                    out.push(r);
                }
                HttpResponse::Ok().json(Rows2d { rows: out })
            }
            Err(e) => HttpResponse::InternalServerError().body(format!("DB error: {}", e)),
        },
        "run" | "execute" => {
            // run/execute -> execute (no returned rows). We'll return an empty rows array per your spec.
            match q.execute(db.get_ref()).await {
                Ok(_res) => HttpResponse::Ok().json(Rows2d { rows: vec![] }),
                Err(e) => HttpResponse::InternalServerError().body(format!("DB error: {}", e)),
            }
        }
        other => HttpResponse::BadRequest().body(format!("unknown method: {}", other)),
    }
}

/// Try a few typed getters to produce a serde_json::Value for any column.
/// This is not exhaustive but handles common scalar types and ensures correct JSON serialization.
fn row_to_value(row: &sqlx::postgres::PgRow, idx: usize) -> Result<Value, sqlx::Error> {
    // Helper macro to handle Option<T> and return Value::Null for None
    macro_rules! try_get_value {
        ($type:ty, $conversion:expr) => {
            if let Ok(v) = row.try_get::<Option<$type>, usize>(idx) {
                return Ok(match v {
                    Some(val) => $conversion(val),
                    None => Value::Null,
                });
            }
        };
    }

    // 1. Try DateTime<Utc> (timestamp with time zone) - serialized as string
    try_get_value!(DateTime<Utc>, |val: DateTime<Utc>| Value::String(
        val.to_string()
    ));

    // 2. Try NaiveDateTime (timestamp without time zone) - serialized as string
    try_get_value!(NaiveDateTime, |val: NaiveDateTime| Value::String(
        val.to_string()
    ));

    // 3. Try String/Text types
    try_get_value!(String, Value::String);

    // 4. Try i64 (Numbers)
    try_get_value!(i64, |val: i64| Value::Number(val.into()));

    // 5. Try i32 (Numbers)
    try_get_value!(i32, |val: i32| Value::Number(val.into()));

    // 6. Try f64 (Numbers)
    try_get_value!(f64, |val: f64| Value::Number(
        serde_json::Number::from_f64(val).unwrap_or(serde_json::Number::from(0))
    ));

    // 7. Try bool (Boolean)
    try_get_value!(bool, Value::Bool);

    // 8. Try JSON value (for json/jsonb)
    try_get_value!(Value, |val: Value| val);

    // 9. Try to read as raw string (for ENUMs, etc.)
    if let Ok(raw) = row.try_get_raw(idx) {
        if !raw.is_null() {
            if let Ok(bytes) = raw.as_bytes() {
                if let Ok(s) = std::str::from_utf8(bytes) {
                    // Check for null bytes to avoid misinterpreting binary data
                    if !s.contains('\0') {
                        return Ok(Value::String(s.to_string()));
                    }
                }
            }
        }
    }

    // As a last resort, attempt to get as bytes and debug print (serialized as string)
    if let Ok(bytes) = row.try_get::<Vec<u8>, usize>(idx) {
        return Ok(Value::String(format!("{:?}", bytes)));
    }

    // If nothing worked, return Null
    Ok(Value::Null)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    // Example: expect DATABASE_URL env var (Postgres URL)
    // e.g. export DATABASE_URL=postgres://user:pass@127.0.0.1/dbname
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to DB");

    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    println!("Listening on http://{}", &bind_addr);
    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(web::Data::new(pool.clone()))
            .route("/exec", web::post().to(execute_handler))
    })
    .bind(bind_addr)?
    .run()
    .await
}
