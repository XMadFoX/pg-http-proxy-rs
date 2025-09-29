use actix_web::{middleware::Logger, web, App, HttpResponse, HttpServer, Responder};
use chrono::{DateTime, NaiveDateTime, Utc};
use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::env;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
struct ProxyRequest {
    sql: String,
    params: Option<Vec<Value>>,
    method: String, // "run" | "all" | "values" | "get" | "execute"
}

#[derive(Debug, Serialize)]
struct Rows2d {
    rows: Vec<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct Row1d {
    rows: Vec<String>,
}

async fn execute_handler(db: web::Data<PgPool>, body: web::Json<ProxyRequest>) -> impl Responder {
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
                        q.bind(s)
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
                _ => q.bind(p.to_string()), // Fallback for other types
            };
        }
    }

    // Limit number of returned rows for safety (example).
    // You can remove or tune as you need; it's a good safety measure.
    // We'll not enforce here hard limit in SQL — caller controls query — so return size-check after fetch.
    match req.method.as_str() {
        "get" => match q.fetch_one(db.get_ref()).await {
            Ok(row) => {
                let mut out = Vec::with_capacity(row.len());
                for (i, _) in row.columns().iter().enumerate() {
                    match row_to_string(&row, i) {
                        Ok(s) => out.push(s),
                        Err(e) => {
                            warn!("column conversion error: {:?}", e);
                            out.push("<<conversion error>>".to_string());
                        }
                    }
                }
                HttpResponse::Ok().json(Row1d { rows: out })
            }
            Err(e) => HttpResponse::InternalServerError().body(format!("DB error: {}", e)),
        },
        "all" | "values" => match q.fetch_all(db.get_ref()).await {
            Ok(rows) => {
                let mut out: Vec<Vec<String>> = Vec::with_capacity(rows.len());
                for row in rows {
                    let mut r: Vec<String> = Vec::with_capacity(row.len());
                    for (i, _) in row.columns().iter().enumerate() {
                        match row_to_string(&row, i) {
                            Ok(s) => r.push(s),
                            Err(e) => {
                                warn!("column conversion error: {:?}", e);
                                r.push("<<conversion error>>".to_string());
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

/// Try a few typed getters to produce a String for any column.
/// This is not exhaustive but handles common scalar types.
/// For production, you'd expand types or use a generic value extractor.
fn row_to_string(row: &sqlx::postgres::PgRow, idx: usize) -> Result<String, sqlx::Error> {
    // 1. Try DateTime<Utc> (timestamp with time zone)
    if let Ok(v) = row.try_get::<Option<DateTime<Utc>>, usize>(idx) {
        return Ok(match v {
            Some(val) => val.to_string(),
            None => "null".to_string(),
        });
    }

    // 2. Try NaiveDateTime (timestamp without time zone)
    if let Ok(v) = row.try_get::<Option<NaiveDateTime>, usize>(idx) {
        return Ok(match v {
            Some(val) => val.to_string(),
            None => "null".to_string(),
        });
    }

    // 3. Try String/Text types
    if let Ok(v) = row.try_get::<Option<String>, usize>(idx) {
        return Ok(match v {
            Some(val) => val,
            None => "null".to_string(),
        });
    }

    // 4. Try i64
    if let Ok(v) = row.try_get::<Option<i64>, usize>(idx) {
        return Ok(match v {
            Some(val) => val.to_string(),
            None => "null".to_string(),
        });
    }

    // 5. Try i32
    if let Ok(v) = row.try_get::<Option<i32>, usize>(idx) {
        return Ok(match v {
            Some(val) => val.to_string(),
            None => "null".to_string(),
        });
    }

    // 6. Try f64
    if let Ok(v) = row.try_get::<Option<f64>, usize>(idx) {
        return Ok(match v {
            Some(val) => val.to_string(),
            None => "null".to_string(),
        });
    }

    // 7. Try bool
    if let Ok(v) = row.try_get::<Option<bool>, usize>(idx) {
        return Ok(match v {
            Some(val) => val.to_string(),
            None => "null".to_string(),
        });
    }

    // 8. Try JSON value (for json/jsonb)
    if let Ok(v) = row.try_get::<Option<serde_json::Value>, usize>(idx) {
        return Ok(match v {
            Some(val) => val.to_string(),
            None => "null".to_string(),
        });
    }

    // As a last resort, attempt to get as bytes and debug print
    if let Ok(bytes) = row.try_get::<Vec<u8>, usize>(idx) {
        return Ok(format!("{:?}", bytes));
    }

    // If nothing worked, return "null" as a final fallback.
    Ok("null".to_string())
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
