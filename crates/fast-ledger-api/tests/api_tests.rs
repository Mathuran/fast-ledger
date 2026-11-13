use axum::body::Body;
use axum::http::{Request, StatusCode};
use fast_ledger_api::FastLedgerService;
use fast_ledger_db::LedgerDb;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn send_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req_builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => req_builder
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => req_builder.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val: Value = serde_json::from_slice(&bytes).unwrap();
    (status, val)
}

#[tokio::test]
async fn http_api_end_to_end_flow() {
    let app = FastLedgerService::new(LedgerDb::new()).router();

    let (status, health) = send_json(&app, "GET", "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["status"], "ok");

    // Create entity
    let (status, ent) = send_json(
        &app,
        "POST",
        "/v1/entities",
        Some(json!({
            "code": "HQ",
            "name": "Fast Ledger HQ",
            "functional_currency": "USD"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let entity_id = ent["id"].as_u64().unwrap();

    // Create accounts: Cash(0), DeferredRev(1), Revenue(2)
    let (_, cash) = send_json(
        &app,
        "POST",
        "/v1/accounts",
        Some(json!({
            "entity_id": entity_id,
            "code": "1000",
            "name": "Cash",
            "account_type": "asset",
            "currency": "USD",
            "cash_flow_category": "operating",
            "is_cash_account": true
        })),
    )
    .await;
    let cash_id = cash["id"].as_u64().unwrap();

    let (_, def_rev) = send_json(
        &app,
        "POST",
        "/v1/accounts",
        Some(json!({
            "entity_id": entity_id,
            "code": "2400",
            "name": "Deferred Revenue",
            "account_type": "liability",
            "currency": "USD",
            "cash_flow_category": "operating"
        })),
    )
    .await;
    let def_id = def_rev["id"].as_u64().unwrap();

    let (_, rev) = send_json(
        &app,
        "POST",
        "/v1/accounts",
        Some(json!({
            "entity_id": entity_id,
            "code": "4000",
            "name": "SaaS Revenue",
            "account_type": "revenue",
            "currency": "USD",
            "cash_flow_category": "operating"
        })),
    )
    .await;
    let rev_id = rev["id"].as_u64().unwrap();

    // Reject unbalanced entry with 400
    let (bad_status, _) = send_json(
        &app,
        "POST",
        "/v1/entries",
        Some(json!({
            "effective_ts_ns": 10,
            "description": "bad",
            "lines": [
                { "account_id": cash_id, "debit": 100, "credit": 0 },
                { "account_id": rev_id, "debit": 0, "credit": 90 }
            ]
        })),
    )
    .await;
    assert_eq!(bad_status, StatusCode::BAD_REQUEST);

    // Post batch entry
    let (batch_status, batch_resp) = send_json(
        &app,
        "POST",
        "/v1/entries/batch",
        Some(json!({
            "entries": [
                {
                    "effective_ts_ns": 50,
                    "description": "Direct sale",
                    "lines": [
                        { "account_id": cash_id, "debit": 5000, "credit": 0 },
                        { "account_id": rev_id, "debit": 0, "credit": 5000 }
                    ]
                }
            ]
        })),
    )
    .await;
    assert_eq!(batch_status, StatusCode::CREATED);
    assert_eq!(batch_resp["tx_ids"].as_array().unwrap().len(), 1);

    // Create 3-period schedule & recognize 2 periods
    let (sched_status, _) = send_json(
        &app,
        "POST",
        "/v1/schedules",
        Some(json!({
            "kind": "deferred_revenue",
            "entity_id": entity_id,
            "currency": "USD",
            "total_amount": 10000,
            "source_account": def_id,
            "target_account": rev_id,
            "initial_funding_account": cash_id,
            "start_ts_ns": 100,
            "period_ns": 100,
            "num_periods": 3,
            "description": "Quarterly SaaS"
        })),
    )
    .await;
    assert_eq!(sched_status, StatusCode::CREATED);

    let (rec_status, rec) = send_json(
        &app,
        "POST",
        "/v1/schedules/recognize",
        Some(json!({ "as_of_ts_ns": 200 })),
    )
    .await;
    assert_eq!(rec_status, StatusCode::OK);
    assert_eq!(rec["recognized"].as_array().unwrap().len(), 2);

    // Check financial reports
    let (_, tb) = send_json(&app, "GET", "/v1/reports/trial-balance", None).await;
    assert_eq!(tb["is_balanced"], true);

    let (_, bs) = send_json(&app, "GET", "/v1/reports/balance-sheet", None).await;
    assert_eq!(bs["is_balanced"], true);
    assert_eq!(bs["total_assets"], 15000);

    let (_, is) = send_json(&app, "GET", "/v1/reports/income-statement", None).await;
    // 5000 + 3333 + 3333 = 11666
    assert_eq!(is["total_revenue"], 11666);

    let (_, cf) = send_json(&app, "GET", "/v1/reports/cash-flow", None).await;
    assert_eq!(cf["is_reconciled"], true);
    assert_eq!(cf["ending_cash"], 15000);

    let (_, bal) = send_json(&app, "GET", &format!("/v1/accounts/{rev_id}/balance"), None).await;
    assert_eq!(bal["normal_balance"], 11666);
}
