use super::*;
fn graph(columns: Value) -> Value {
    json!({"status":"loaded","series":["Messages","Views"],"columns":columns})
}
#[test]
fn daily_total_and_previous_period() {
    let g = graph(json!([
        ["x", 0, 86400000, 172800000, 259200000],
        ["y0", 1, 2, 3, 4]
    ]));
    let r = daily("messages_graph", &g, 2);
    assert_eq!(r["series"]["Messages"]["total"], 7.0);
    assert_eq!(r["series"]["Messages"]["previous_total"], 3.0);
    assert_eq!(r["series"]["Messages"]["change_percent"], 133.3);
}
#[test]
fn level_graphs_report_change_instead_of_sum() {
    let g = graph(json!([["x", 0, 1, 2, 3], ["y0", 100, 110, 120, 150]]));
    let r = daily("growth_graph", &g, 2);
    assert_eq!(r["series"]["Messages"]["start"], 120.0);
    assert_eq!(r["series"]["Messages"]["end"], 150.0);
    assert_eq!(r["series"]["Messages"]["change"], 30.0);
    assert_eq!(r["series"]["Messages"]["previous_change"], 10.0);
    assert!(r["series"]["Messages"].get("total").is_none());
}

#[test]
fn follower_graph_sums_relative_changes_instead_of_subtracting_endpoints() {
    let g = graph(json!([["x", 0, 86400000, 172800000], ["y0", 2, 5, 3]]));
    let result = daily("followers_graph", &g, 3);
    assert_eq!(result["series"]["Messages"]["total"], 10.0);
    assert!(result["series"]["Messages"].get("change").is_none());
}
#[test]
fn too_short_history_does_not_invent_previous_total() {
    let g = graph(json!([["x", 0, 86400000], ["y0", 3, 4]]));
    let r = daily("messages_graph", &g, 30);
    assert!(r["series"]["Messages"]["previous_total"].is_null());
    assert!(r["note"].is_string());
}
#[test]
fn series_share_is_against_combined_total() {
    let g = graph(json!([["x", 0, 1], ["y0", 3, 3], ["y1", 1, 1]]));
    let r = daily("messages_graph", &g, 2);
    assert_eq!(r["series"]["Messages"]["share_percent"], 75.0);
    assert_eq!(r["series"]["Views"]["share_percent"], 25.0);
}
#[test]
fn zero_denominator_is_not_infinite() {
    assert!(percent(12.0, 0.0).is_null());
}
#[test]
fn empty_hour_graph_is_empty() {
    assert_eq!(hours(&graph(json!([["x"]])))["status"], "empty");
}
#[test]
fn hour_graph_reports_utc_peaks() {
    let g = graph(json!([["x", 0, 1, 2], ["y0", 5, 20, 2]]));
    let r = hours(&g);
    assert_eq!(r["busiest_hour"], 1);
    assert_eq!(r["quietest_hour"], 2);
    assert_eq!(r["note"], "Hours are in UTC.");
}
#[test]
fn analysis_omits_raw_columns_by_default() {
    let g = graph(json!([["x", 0], ["y0", 5]]));
    let r = analyze(&json!({"messages_graph":g}), 1, false);
    assert!(r["messages_graph"].get("columns").is_none());
}
#[test]
fn analysis_keeps_requested_raw_columns() {
    let g = graph(json!([["x", 0], ["y0", 5]]));
    let r = analyze(&json!({"messages_graph":g}), 1, true);
    assert!(r["messages_graph"]["columns"].is_array());
}
#[test]
fn unavailable_graph_remains_unavailable() {
    let r = analyze(&json!({"messages_graph":{"status":"not_loaded"}}), 1, false);
    assert_eq!(r["messages_graph"]["status"], "not_loaded");
}

struct GraphApi {
    response: Option<Value>,
    calls: std::sync::Mutex<Vec<(String, Value)>>,
}
impl GraphApi {
    fn new(response: Option<Value>) -> Self {
        Self {
            response,
            calls: std::sync::Mutex::new(vec![]),
        }
    }
}
#[async_trait::async_trait]
impl Telegram for GraphApi {
    async fn invoke(&self, method: &str, args: Value) -> Result<Value> {
        crate::tl::Schema::global().encode_call(method, &args)?;
        self.calls.lock().unwrap().push((method.into(), args));
        self.response
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Graph unavailable"))
    }
    async fn resolve_peer(&self, _: &Value) -> Result<Value> {
        anyhow::bail!("No peer lookup expected")
    }
}
fn wire_graph(data: Value) -> Value {
    json!({"_":"statsGraph","json":{"_":"dataJSON","data":data.to_string()}})
}

#[tokio::test]
async fn async_graph_failures_and_remote_errors_preserve_unavailable_state() {
    let api = GraphApi::new(None);
    let missing = super::graph(&api, &json!({"_":"statsGraphAsync"}), true, false).await;
    assert!(missing.unwrap_err().to_string().contains("token"));
    let failed = super::graph(
        &api,
        &json!({"_":"statsGraphAsync","token":"opaque-sensitive-token"}),
        true,
        false,
    )
    .await
    .unwrap();
    assert_eq!(failed["status"], "not_loaded");
    assert!(!failed.to_string().contains("opaque-sensitive-token"));
    assert_eq!(api.calls.lock().unwrap().len(), 1);
    let remote = super::graph(
        &api,
        &json!({"_":"statsGraphError","error":"GRAPH_EXPIRED"}),
        true,
        false,
    )
    .await
    .unwrap();
    assert_eq!(remote, json!({"status":"error","error":"GRAPH_EXPIRED"}));
}

#[tokio::test]
async fn incomplete_or_corrupt_graph_data_is_not_reported_loaded() {
    let api = GraphApi::new(None);
    assert_eq!(
        super::graph(&api, &json!({"_":"unknownGraph"}), true, false)
            .await
            .unwrap()["status"],
        "not_loaded"
    );
    assert!(
        super::graph(&api, &json!({"_":"statsGraph","json":{}}), true, false)
            .await
            .is_err()
    );
    let broken = json!({"_":"statsGraph","json":{"data":"{broken-json"}});
    assert_eq!(
        super::graph(&api, &broken, true, false).await.unwrap()["status"],
        "error"
    );
}

#[tokio::test]
async fn malformed_graph_columns_are_rejected_before_analysis_can_slice_them() {
    let api = GraphApi::new(None);
    for columns in [
        json!([[]]),
        json!([[null, 1]]),
        json!([{"x":1}]),
        json!([[42, 1]]),
    ] {
        let error = super::graph(&api, &wire_graph(json!({"columns":columns})), true, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("malformed columns"));
    }
}

#[tokio::test]
async fn graph_decoding_enforces_byte_series_and_point_budgets() {
    let api = GraphApi::new(None);
    let oversized = json!({"_":"statsGraph","json":{"data":" ".repeat(8*1024*1024+1)}});
    assert!(
        super::graph(&api, &oversized, true, false)
            .await
            .unwrap_err()
            .to_string()
            .contains("8 MiB")
    );
    let many = wire_graph(json!({"columns":vec![json!(["series",1]);201]}));
    assert!(
        super::graph(&api, &many, true, false)
            .await
            .unwrap_err()
            .to_string()
            .contains("point budget")
    );
    let mut long = vec![json!("x")];
    long.extend(std::iter::repeat_n(json!(1), 100001));
    let points = wire_graph(json!({"columns":[long]}));
    assert!(
        super::graph(&api, &points, true, false)
            .await
            .unwrap_err()
            .to_string()
            .contains("point budget")
    );
}

#[tokio::test]
async fn graph_without_display_names_uses_series_identifiers_and_empty_graph_stays_empty() {
    let api = GraphApi::new(None);
    let loaded = super::graph(
        &api,
        &wire_graph(json!({"columns":[["x",0,1000],["visits",2,7]]})),
        true,
        false,
    )
    .await
    .unwrap();
    assert_eq!(loaded["series"], json!(["visits"]));
    assert_eq!(loaded["points"], 2);
    assert_eq!(loaded["from"], "1970-01-01T00:00:00+00:00");
    assert!(loaded.get("columns").is_none());
    let empty = super::graph(&api, &wire_graph(json!({"columns":[]})), true, true)
        .await
        .unwrap();
    assert_eq!(empty["points"], 0);
    assert_eq!(empty["series"], json!([]));
    assert_eq!(empty["columns"], json!([]));
    assert!(empty.get("from").is_none());
}

#[tokio::test]
async fn normalized_counters_and_leaderboards_keep_zero_denominators_and_deleted_accounts_explicit()
{
    let api = GraphApi::new(None);
    let mut input = json!({"empty":{"_":"statsPercentValue","part":0.0,"total":0.0},
        "positive":{"_":"statsPercentValue","part":1.0,"total":3.0},
        "users":[{"id":"42","first_name":"نام","last_name":"کامل"},{"id":"43","deleted":true}],
        "top_posters":[{"user_id":"42"},{"user_id":"43"},{"user_id":"404"}],
        "top_admins":[{"user_id":"42"}],"top_inviters":[{"user_id":"43"}]});
    resolve_graphs(&api, &mut input, true, false).await.unwrap();
    assert!(input["empty"]["percent"].is_null());
    assert_eq!(input["positive"]["percent"], 33.33);
    assert_eq!(input["top_posters"][0]["name"], "نام کامل");
    assert_eq!(input["top_posters"][1]["name"], "Deleted Account");
    assert_eq!(input["top_posters"][1]["deleted_account"], true);
    assert!(input["top_posters"][2].get("name").is_none());
    assert_eq!(input["top_admins"][0]["name"], "نام کامل");
    assert_eq!(input["top_inviters"][0]["deleted_account"], true);
}

#[test]
fn zero_daily_activity_has_no_invented_share_and_short_series_align_to_latest_dates() {
    let empty = graph(json!([["x", 0, 86400000], ["y0", 0, 0], ["y1", 0, 0]]));
    let analyzed = daily("messages_graph", &empty, 2);
    assert!(analyzed["series"]["Messages"]["share_percent"].is_null());
    let short = graph(json!([["x", 0, 86400000, 172800000], ["y0", 5]]));
    let analyzed = daily("messages_graph", &short, 3);
    assert_eq!(analyzed["series"]["Messages"]["peak"]["date"], "1970-01-03");
    assert_eq!(analyzed["series"]["Messages"]["total"], 5.0);
}

#[test]
fn hourly_comparison_reports_previous_total_and_empty_series_without_guessing_peaks() {
    assert_eq!(hours(&graph(json!([["x", 0], ["y0"]])))["status"], "empty");
    let result = hours(&graph(json!([["x", 0, 1], ["y0", 3, 5], ["y1", 1, 3]])));
    assert_eq!(result["previous_period"], "Views");
    assert_eq!(result["change_percent"], 100.0);
    let result = hours(&graph(json!([["x", 0], ["y0", 5], ["y1", 0]])));
    assert!(result["change_percent"].is_null());
}

#[test]
fn weekday_analysis_uses_only_the_requested_recent_weeks() {
    let graph = json!({"status":"loaded","series":["Monday","Friday"],
        "columns":[["x",0,1,2],["m",100,2,3],["f",0,8,7]]});
    let result = weekdays(&graph, 8);
    assert_eq!(result["weeks"], 2);
    assert_eq!(result["totals"]["Monday"], 5.0);
    assert_eq!(result["totals"]["Friday"], 15.0);
    assert_eq!(result["share_percent"]["Monday"], 25.0);
    assert_eq!(result["busiest_weekday"], "Friday");
    assert_eq!(result["quietest_weekday"], "Monday");
    assert_eq!(weekdays(&json!({}), 30)["status"], "empty");
    assert!(weekdays(&json!({"series":["Monday"],"columns":[["x",0],["m",0]]}),1)["share_percent"]["Monday"].is_null());
}

#[test]
fn complete_analysis_dispatches_counter_hour_and_weekday_helpers_without_losing_requested_series() {
    let input = json!({"counter":{"current":15.0,"previous":10.0},
        "top_hours_graph":{"status":"loaded","series":["Now"],"columns":[["x",0,1],["y",1,3]]},
        "weekdays_graph":{"status":"loaded","series":["Monday"],"columns":[["x",0],["m",4]]}});
    let result = analyze(&input, 7, true);
    assert_eq!(result["counter"]["change_percent"], 50.0);
    assert_eq!(result["top_hours_graph"]["busiest_hour"], 1);
    assert_eq!(result["weekdays_graph"]["totals"]["Monday"], 4.0);
    assert_eq!(
        result["weekdays_graph"]["columns"],
        input["weekdays_graph"]["columns"]
    );
    assert_eq!(moment(&json!("not-a-timestamp")), json!("not-a-timestamp"));
    assert_eq!(moment(&json!(i64::MAX)), json!(i64::MAX));
}
