use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use fast_ledger_core::{
    AccountId, AmortizationSchedule, CreateAccountInput, CreateEntityInput, Currency, Entity,
    EntityId, IntercompanyRule, JournalEntryInput, LedgerError, ScheduleId, ScheduleSpec,
    TimestampNs, TxId,
};
use fast_ledger_db::{
    generate_balance_sheet, generate_cash_flow_statement, generate_consolidated_report,
    generate_income_statement, generate_trial_balance, AccountRecord, BalanceSnapshot, FxRateTable,
    LedgerDb, RecognitionResult,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

/// Shared thread-safe service handle wrapping `LedgerDb`.
#[derive(Clone)]
pub struct FastLedgerService {
    pub db: Arc<RwLock<LedgerDb>>,
}

impl FastLedgerService {
    pub fn new(db: LedgerDb) -> Self {
        Self {
            db: Arc::new(RwLock::new(db)),
        }
    }

    pub fn router(self) -> Router {
        build_router(self)
    }
}

pub fn build_router(service: FastLedgerService) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/entities", post(create_entity).get(list_entities))
        .route("/v1/accounts", post(create_account).get(list_accounts))
        .route("/v1/accounts/:id/balance", get(get_account_balance))
        .route("/v1/intercompany", post(register_intercompany))
        .route("/v1/entries", post(post_entry))
        .route("/v1/entries/batch", post(post_batch))
        .route("/v1/schedules", post(create_schedule).get(list_schedules))
        .route("/v1/schedules/recognize", post(recognize_schedules))
        .route("/v1/reports/trial-balance", get(trial_balance))
        .route("/v1/reports/balance-sheet", get(balance_sheet))
        .route("/v1/reports/income-statement", get(income_statement))
        .route("/v1/reports/cash-flow", get(cash_flow))
        .route("/v1/reports/consolidated", get(consolidated_report))
        .with_state(service)
}

struct ApiError(StatusCode, String);

impl From<LedgerError> for ApiError {
    fn from(err: LedgerError) -> Self {
        let code = match &err {
            LedgerError::UnknownAccount(_)
            | LedgerError::UnknownEntity(_)
            | LedgerError::UnknownSchedule(_) => StatusCode::NOT_FOUND,
            LedgerError::DuplicateEntityCode(_) | LedgerError::DuplicateAccountCode { .. } => {
                StatusCode::CONFLICT
            }
            LedgerError::Io(_) | LedgerError::WalCorruption(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        Self(code, err.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorBody {
            error: String,
        }
        (self.0, Json(ErrorBody { error: self.1 })).into_response()
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    entities: usize,
    accounts: usize,
    tx_count: usize,
    line_count: usize,
    global_net_delta: i128,
}

async fn health(State(svc): State<FastLedgerService>) -> Result<Json<HealthResponse>, ApiError> {
    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    Ok(Json(HealthResponse {
        status: "ok",
        entities: db.entities().len(),
        accounts: db.accounts().len(),
        tx_count: db.tx_count(),
        line_count: db.line_count(),
        global_net_delta: db.global_net_delta(),
    }))
}

#[derive(Serialize)]
struct CreateEntityResponse {
    id: EntityId,
}

async fn create_entity(
    State(svc): State<FastLedgerService>,
    Json(input): Json<CreateEntityInput>,
) -> Result<(StatusCode, Json<CreateEntityResponse>), ApiError> {
    let mut db = svc
        .db
        .write()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let id = db.create_entity(input)?;
    Ok((StatusCode::CREATED, Json(CreateEntityResponse { id })))
}

async fn list_entities(
    State(svc): State<FastLedgerService>,
) -> Result<Json<Vec<Entity>>, ApiError> {
    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    Ok(Json(db.entities().to_vec()))
}

#[derive(Serialize)]
struct CreateAccountResponse {
    id: AccountId,
}

async fn create_account(
    State(svc): State<FastLedgerService>,
    Json(input): Json<CreateAccountInput>,
) -> Result<(StatusCode, Json<CreateAccountResponse>), ApiError> {
    let mut db = svc
        .db
        .write()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let id = db.create_account(input)?;
    Ok((StatusCode::CREATED, Json(CreateAccountResponse { id })))
}

async fn list_accounts(
    State(svc): State<FastLedgerService>,
) -> Result<Json<Vec<AccountRecord>>, ApiError> {
    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    Ok(Json(db.accounts().to_vec()))
}

#[derive(Deserialize)]
struct BalanceQuery {
    as_of_ts_ns: Option<u64>,
    start_ts_ns: Option<u64>,
    end_ts_ns: Option<u64>,
}

async fn get_account_balance(
    State(svc): State<FastLedgerService>,
    Path(id): Path<u32>,
    Query(q): Query<BalanceQuery>,
) -> Result<Json<BalanceSnapshot>, ApiError> {
    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let snap = match (q.start_ts_ns, q.end_ts_ns) {
        (Some(start), Some(end)) => {
            db.activity_between(AccountId(id), TimestampNs(start), TimestampNs(end))?
        }
        _ => match q.as_of_ts_ns {
            Some(ts) => db.balance_as_of(AccountId(id), TimestampNs(ts))?,
            None => db.current_balance(AccountId(id))?,
        },
    };
    Ok(Json(snap))
}

#[derive(Serialize)]
struct OkResponse {
    ok: bool,
}

async fn register_intercompany(
    State(svc): State<FastLedgerService>,
    Json(rule): Json<IntercompanyRule>,
) -> Result<Json<OkResponse>, ApiError> {
    let mut db = svc
        .db
        .write()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    db.register_intercompany_rule(rule)?;
    Ok(Json(OkResponse { ok: true }))
}

#[derive(Serialize)]
struct PostEntryResponse {
    tx_id: TxId,
}

async fn post_entry(
    State(svc): State<FastLedgerService>,
    Json(input): Json<JournalEntryInput>,
) -> Result<(StatusCode, Json<PostEntryResponse>), ApiError> {
    let mut db = svc
        .db
        .write()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let tx_id = db.post_entry(input)?;
    Ok((StatusCode::CREATED, Json(PostEntryResponse { tx_id })))
}

#[derive(Deserialize)]
struct PostBatchRequest {
    entries: Vec<JournalEntryInput>,
}

#[derive(Serialize)]
struct PostBatchResponse {
    tx_ids: Vec<TxId>,
}

async fn post_batch(
    State(svc): State<FastLedgerService>,
    Json(req): Json<PostBatchRequest>,
) -> Result<(StatusCode, Json<PostBatchResponse>), ApiError> {
    let mut db = svc
        .db
        .write()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let tx_ids = db.post_batch(req.entries)?;
    Ok((StatusCode::CREATED, Json(PostBatchResponse { tx_ids })))
}

#[derive(Serialize)]
struct CreateScheduleResponse {
    schedule_id: ScheduleId,
}

async fn create_schedule(
    State(svc): State<FastLedgerService>,
    Json(spec): Json<ScheduleSpec>,
) -> Result<(StatusCode, Json<CreateScheduleResponse>), ApiError> {
    let mut db = svc
        .db
        .write()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let schedule_id = db.create_schedule(spec)?;
    Ok((
        StatusCode::CREATED,
        Json(CreateScheduleResponse { schedule_id }),
    ))
}

async fn list_schedules(
    State(svc): State<FastLedgerService>,
) -> Result<Json<Vec<AmortizationSchedule>>, ApiError> {
    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    Ok(Json(db.schedules().to_vec()))
}

#[derive(Deserialize)]
struct RecognizeRequest {
    as_of_ts_ns: u64,
}

#[derive(Serialize)]
struct RecognizeResponse {
    recognized: Vec<RecognitionResult>,
}

async fn recognize_schedules(
    State(svc): State<FastLedgerService>,
    Json(req): Json<RecognizeRequest>,
) -> Result<Json<RecognizeResponse>, ApiError> {
    let mut db = svc
        .db
        .write()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let recognized = db.recognize_due_schedules(TimestampNs(req.as_of_ts_ns))?;
    Ok(Json(RecognizeResponse { recognized }))
}

#[derive(Deserialize)]
struct PointInTimeReportQuery {
    entity_id: Option<u16>,
    as_of_ts_ns: Option<u64>,
}

async fn trial_balance(
    State(svc): State<FastLedgerService>,
    Query(q): Query<PointInTimeReportQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let report = generate_trial_balance(
        &db,
        q.entity_id.map(EntityId),
        TimestampNs(q.as_of_ts_ns.unwrap_or(u64::MAX)),
    )?;
    Ok(Json(report))
}

async fn balance_sheet(
    State(svc): State<FastLedgerService>,
    Query(q): Query<PointInTimeReportQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let report = generate_balance_sheet(
        &db,
        q.entity_id.map(EntityId),
        TimestampNs(q.as_of_ts_ns.unwrap_or(u64::MAX)),
    )?;
    Ok(Json(report))
}

#[derive(Deserialize)]
struct PeriodReportQuery {
    entity_id: Option<u16>,
    start_ts_ns: Option<u64>,
    end_ts_ns: Option<u64>,
}

async fn income_statement(
    State(svc): State<FastLedgerService>,
    Query(q): Query<PeriodReportQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let report = generate_income_statement(
        &db,
        q.entity_id.map(EntityId),
        TimestampNs(q.start_ts_ns.unwrap_or(0)),
        TimestampNs(q.end_ts_ns.unwrap_or(u64::MAX)),
    )?;
    Ok(Json(report))
}

async fn cash_flow(
    State(svc): State<FastLedgerService>,
    Query(q): Query<PeriodReportQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let report = generate_cash_flow_statement(
        &db,
        q.entity_id.map(EntityId),
        TimestampNs(q.start_ts_ns.unwrap_or(0)),
        TimestampNs(q.end_ts_ns.unwrap_or(u64::MAX)),
    )?;
    Ok(Json(report))
}

#[derive(Deserialize)]
struct ConsolidatedQuery {
    presentation_currency: Option<String>,
    as_of_ts_ns: Option<u64>,
    start_ts_ns: Option<u64>,
    end_ts_ns: Option<u64>,
    /// Comma-separated FX rules: `EUR:USD=1100000,GBP:USD=1270000`
    rates: Option<String>,
}

async fn consolidated_report(
    State(svc): State<FastLedgerService>,
    Query(q): Query<ConsolidatedQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let presentation_currency = match q.presentation_currency {
        Some(code) => Currency::from_str(&code)?,
        None => Currency::USD,
    };

    let mut fx_table = FxRateTable::new();
    if let Some(rates_str) = q.rates {
        for pair in rates_str.split(',').filter(|s| !s.trim().is_empty()) {
            let (currencies, rate_str) = pair.split_once('=').ok_or_else(|| {
                ApiError(
                    StatusCode::BAD_REQUEST,
                    format!("invalid FX rate specification '{pair}'; expected FROM:TO=SCALED_INT"),
                )
            })?;
            let (from_str, to_str) = currencies.split_once(':').ok_or_else(|| {
                ApiError(
                    StatusCode::BAD_REQUEST,
                    format!("invalid currency pair '{currencies}'; expected FROM:TO"),
                )
            })?;
            let from = Currency::from_str(from_str.trim())?;
            let to = Currency::from_str(to_str.trim())?;
            let rate_scaled: i128 = rate_str.trim().parse().map_err(|_| {
                ApiError(
                    StatusCode::BAD_REQUEST,
                    format!("invalid integer rate '{rate_str}'"),
                )
            })?;
            fx_table.set_rate(from, to, rate_scaled)?;
        }
    }

    let db = svc
        .db
        .read()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "lock poisoned".into()))?;
    let report = generate_consolidated_report(
        &db,
        presentation_currency,
        TimestampNs(q.as_of_ts_ns.unwrap_or(u64::MAX)),
        TimestampNs(q.start_ts_ns.unwrap_or(0)),
        TimestampNs(q.end_ts_ns.unwrap_or(u64::MAX)),
        &fx_table,
    )?;
    Ok(Json(report))
}
