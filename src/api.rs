use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::client::{self, DepError};

pub const SERVICE: &str = "srvcs-iseven";
pub const CONCERN: &str = "parity: is the number even";
pub const DEPENDS_ON: &[&str] = &["srvcs-isnumber"];

/// Dependency endpoints, injected as router state so tests can point them at
/// mock services.
#[derive(Clone)]
pub struct Deps {
    pub isnumber_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Info {
    pub service: &'static str,
    pub concern: &'static str,
    pub depends_on: Vec<&'static str>,
}

/// `GET /` — service identity (srvcs service standard).
#[utoipa::path(get, path = "/", responses((status = 200, body = Info)))]
pub async fn index() -> Json<Info> {
    Json(Info {
        service: SERVICE,
        concern: CONCERN,
        depends_on: DEPENDS_ON.to_vec(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct EvalRequest {
    #[schema(value_type = Object)]
    pub value: Value,
}

#[derive(Serialize, ToSchema)]
pub struct PredicateResponse {
    #[schema(value_type = Object)]
    pub value: Value,
    pub result: bool,
}

/// Coerce a validated numeric JSON value to an integer, accepting whole floats
/// (`4.0`) but rejecting genuinely fractional ones (`4.5`).
fn as_integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|f| f.fract() == 0.0)
            .map(|f| f as i64)
    })
}

/// The single concern: is the integer even?
pub fn is_even(n: i64) -> bool {
    n % 2 == 0
}

fn ok(value: Value, result: bool) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "value": value, "result": result })),
    )
        .into_response()
}

fn invalid(reason: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": reason })),
    )
        .into_response()
}

fn degraded(dependency: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "dependency unavailable", "dependency": dependency })),
    )
        .into_response()
}

/// `POST /` — is `value` even?
///
/// Input validation is delegated to `srvcs-isnumber` over HTTP (the single
/// source of truth for "is this a number"). If that dependency is unreachable,
/// this service reports itself degraded rather than guessing.
#[utoipa::path(
    post,
    path = "/",
    request_body = EvalRequest,
    responses(
        (status = 200, body = PredicateResponse),
        (status = 422, description = "value is not an integer"),
        (status = 503, description = "a dependency is unavailable")
    )
)]
pub async fn evaluate(State(deps): State<Deps>, Json(req): Json<EvalRequest>) -> Response {
    // 1. Delegate "is this a number" to srvcs-isnumber.
    match client::evaluate_dep(&deps.isnumber_url, &req.value).await {
        Err(DepError::Unreachable) => return degraded("srvcs-isnumber"),
        Ok((200, body)) => {
            let is_number = body.get("result").and_then(Value::as_bool).unwrap_or(false);
            if !is_number {
                return invalid("value is not a number");
            }
        }
        Ok(_) => return degraded("srvcs-isnumber"),
    }

    // 2. Parity is defined on integers.
    let Some(n) = as_integer(&req.value) else {
        return invalid("value is not an integer");
    };

    ok(req.value, is_even(n))
}

#[derive(OpenApi)]
#[openapi(
    paths(index, evaluate),
    components(schemas(Info, EvalRequest, PredicateResponse))
)]
pub struct ApiDoc;

/// Serve OpenAPI document
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_documents_routes() {
        let doc = ApiDoc::openapi();
        let root = doc.paths.paths.get("/").expect("path / present");
        assert!(root.get.is_some());
        assert!(root.post.is_some());
    }

    #[test]
    fn parity_is_correct() {
        for n in [0, 2, -4, 100, i64::MIN] {
            assert!(is_even(n), "{n} is even");
        }
        for n in [1, 3, -5, 99, i64::MAX] {
            assert!(!is_even(n), "{n} is odd");
        }
    }

    #[test]
    fn whole_floats_are_integers_but_fractions_are_not() {
        assert_eq!(as_integer(&json!(4)), Some(4));
        assert_eq!(as_integer(&json!(4.0)), Some(4));
        assert_eq!(as_integer(&json!(-6.0)), Some(-6));
        assert_eq!(as_integer(&json!(4.5)), None);
    }
}
