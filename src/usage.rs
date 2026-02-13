use std::sync::Arc;

use crate::config::Config;
use crate::db::{call_blocking, Database, LlmModelUsageSummary, LlmUsageSummary};

struct CostEstimate {
    usd: f64,
    missing_models: Vec<String>,
}

fn estimate_cost(config: &Config, rows: &[LlmModelUsageSummary]) -> CostEstimate {
    let mut usd = 0.0f64;
    let mut missing = Vec::new();
    for row in rows {
        if let Some(cost) =
            config.estimate_cost_usd(&row.model, row.input_tokens, row.output_tokens)
        {
            usd += cost;
        } else {
            missing.push(row.model.clone());
        }
    }
    CostEstimate {
        usd,
        missing_models: missing,
    }
}

fn fmt_money(usd: f64) -> String {
    format!("${usd:.4}")
}

fn fmt_summary_line(name: &str, s: &LlmUsageSummary, cost: &CostEstimate) -> String {
    let mut line = format!(
        "{name}: req={}, in={}, out={}, total={}, est_cost={}",
        s.requests,
        s.input_tokens,
        s.output_tokens,
        s.total_tokens,
        fmt_money(cost.usd)
    );
    if !cost.missing_models.is_empty() {
        let mut uniq = cost.missing_models.clone();
        uniq.sort();
        uniq.dedup();
        line.push_str(&format!(" (unpriced models: {})", uniq.join(", ")));
    }
    line
}

fn format_model_rows(
    config: &Config,
    rows: &[LlmModelUsageSummary],
    max_rows: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    for row in rows.iter().take(max_rows) {
        let cost = config
            .estimate_cost_usd(&row.model, row.input_tokens, row.output_tokens)
            .map(fmt_money)
            .unwrap_or_else(|| "n/a".to_string());
        out.push(format!(
            "- {}: req={}, in={}, out={}, total={}, est_cost={}",
            row.model, row.requests, row.input_tokens, row.output_tokens, row.total_tokens, cost
        ));
    }
    out
}

async fn query_summary(
    db: Arc<Database>,
    chat_id: Option<i64>,
    since: Option<String>,
) -> Result<LlmUsageSummary, String> {
    call_blocking(db, move |d| {
        d.get_llm_usage_summary_since(chat_id, since.as_deref())
    })
    .await
    .map_err(|e| e.to_string())
}

async fn query_by_model(
    db: Arc<Database>,
    chat_id: Option<i64>,
    since: Option<String>,
) -> Result<Vec<LlmModelUsageSummary>, String> {
    call_blocking(db, move |d| {
        d.get_llm_usage_by_model(chat_id, since.as_deref(), None)
    })
    .await
    .map_err(|e| e.to_string())
}

pub async fn build_usage_report(
    db: Arc<Database>,
    config: &Config,
    chat_id: i64,
) -> Result<String, String> {
    let now = chrono::Utc::now();
    let since_24h = (now - chrono::Duration::hours(24)).to_rfc3339();
    let since_7d = (now - chrono::Duration::days(7)).to_rfc3339();

    let chat_all = query_summary(db.clone(), Some(chat_id), None).await?;
    let chat_24h = query_summary(db.clone(), Some(chat_id), Some(since_24h.clone())).await?;
    let chat_7d = query_summary(db.clone(), Some(chat_id), Some(since_7d.clone())).await?;
    let chat_models_all = query_by_model(db.clone(), Some(chat_id), None).await?;
    let chat_models_24h = query_by_model(db.clone(), Some(chat_id), Some(since_24h)).await?;
    let chat_models_7d = query_by_model(db.clone(), Some(chat_id), Some(since_7d)).await?;

    let global_all = query_summary(db.clone(), None, None).await?;
    let global_24h = query_summary(
        db.clone(),
        None,
        Some((now - chrono::Duration::hours(24)).to_rfc3339()),
    )
    .await?;
    let global_7d = query_summary(
        db.clone(),
        None,
        Some((now - chrono::Duration::days(7)).to_rfc3339()),
    )
    .await?;
    let global_models_all = query_by_model(db.clone(), None, None).await?;
    let global_models_24h = query_by_model(
        db.clone(),
        None,
        Some((now - chrono::Duration::hours(24)).to_rfc3339()),
    )
    .await?;
    let global_models_7d = query_by_model(
        db,
        None,
        Some((now - chrono::Duration::days(7)).to_rfc3339()),
    )
    .await?;

    let chat_cost_all = estimate_cost(config, &chat_models_all);
    let chat_cost_24h = estimate_cost(config, &chat_models_24h);
    let chat_cost_7d = estimate_cost(config, &chat_models_7d);
    let global_cost_all = estimate_cost(config, &global_models_all);
    let global_cost_24h = estimate_cost(config, &global_models_24h);
    let global_cost_7d = estimate_cost(config, &global_models_7d);

    let mut lines = vec![
        "Token usage stats".to_string(),
        format!("Now: {}", now.to_rfc3339()),
        "".to_string(),
        "[This chat]".to_string(),
        fmt_summary_line("All-time", &chat_all, &chat_cost_all),
        fmt_summary_line("Last 24h", &chat_24h, &chat_cost_24h),
        fmt_summary_line("Last 7d", &chat_7d, &chat_cost_7d),
        "By model (last 24h):".to_string(),
    ];

    let chat_model_lines_24h = format_model_rows(config, &chat_models_24h, 6);
    if chat_model_lines_24h.is_empty() {
        lines.push("- (no data)".to_string());
    } else {
        lines.extend(chat_model_lines_24h);
    }

    lines.extend(["By model (last 7d):".to_string()]);

    let chat_model_lines = format_model_rows(config, &chat_models_7d, 6);
    if chat_model_lines.is_empty() {
        lines.push("- (no data)".to_string());
    } else {
        lines.extend(chat_model_lines);
    }

    lines.push("".to_string());
    lines.push("[Global]".to_string());
    lines.push(fmt_summary_line("All-time", &global_all, &global_cost_all));
    lines.push(fmt_summary_line("Last 24h", &global_24h, &global_cost_24h));
    lines.push(fmt_summary_line("Last 7d", &global_7d, &global_cost_7d));
    lines.push("By model (last 24h):".to_string());

    let global_model_lines_24h = format_model_rows(config, &global_models_24h, 6);
    if global_model_lines_24h.is_empty() {
        lines.push("- (no data)".to_string());
    } else {
        lines.extend(global_model_lines_24h);
    }

    lines.push("By model (last 7d):".to_string());

    let global_model_lines = format_model_rows(config, &global_models_7d, 6);
    if global_model_lines.is_empty() {
        lines.push("- (no data)".to_string());
    } else {
        lines.extend(global_model_lines);
    }

    Ok(lines.join("\n"))
}
