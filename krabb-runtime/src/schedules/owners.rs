//! Each schedule belongs to the scheduler that holds it: lists merge across every scheduler,
//! and controls reach the owner.

use super::*;

/// Pages searched in each scheduler for a schedule's owner.
const OWNER_PAGES: u64 = 64;
const OWNER_CALL: std::time::Duration = std::time::Duration::from_secs(5);

/// Joins one list page from each scheduler; every item names the scheduler that holds it.
pub(crate) fn merge(pages: Vec<(String, Value)>) -> Value {
    let mut items = Vec::new();
    let mut total = 0_u64;
    let mut count = 0_u64;
    let mut skipped = 0_u64;
    let mut page = 1;

    for (scheduler, value) in pages {
        total = total.saturating_add(value["total"].as_u64().unwrap_or_default());
        count = count.max(value["pages"].as_u64().unwrap_or(1));
        page = value["page"].as_u64().unwrap_or(page);
        skipped = skipped.saturating_add(value["from"].as_u64().unwrap_or(1).saturating_sub(1));

        for item in value["items"].as_array().into_iter().flatten() {
            let mut item = item.clone();

            item["scheduler"] = scheduler.as_str().into();
            items.push(item);
        }
    }

    let from = if items.is_empty() { 0 } else { skipped + 1 };

    let to = skipped + items.len() as u64;

    serde_json::json!({
        "items": items, "total": total, "page": page, "pages": count.max(1), "from": from, "to": to,
    })
}

/// Lists one page from every scheduler through `method`, `list` or a caller's `control`. One
/// scheduler answers as it always has; with more, a page beyond one's range only drops it.
pub(crate) async fn list(
    plugins: &Plugins,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let members = plugins.members(&Capability::Scheduler).await;

    if members.also.is_empty() {
        let (_, scheduler) = members.primary.ok_or("The scheduler plugin is not installed.")?;

        return answer(&scheduler, id, method, params).await;
    }

    let mut pages = Vec::new();
    let mut failure = None;

    for (scheduler_id, scheduler) in members.all() {
        match answer(scheduler, id, method, params.clone()).await {
            Ok(value) => pages.push((scheduler_id.clone(), value)),
            Err(error) => failure = Some(error),
        }
    }

    match (pages.is_empty(), failure) {
        (true, Some(error)) => Err(error),
        _ => Ok(merge(pages)),
    }
}

/// The scheduler holding schedule `id`, or the primary when none lists it.
pub(crate) async fn owner(plugins: &Plugins, id: u64) -> Option<(String, Arc<Live>)> {
    let members = plugins.members(&Capability::Scheduler).await;

    if members.also.is_empty() {
        return members.primary;
    }

    for (scheduler_id, scheduler) in members.all() {
        if holds(scheduler, id).await {
            return Some((scheduler_id.clone(), Arc::clone(scheduler)));
        }
    }

    members.primary
}

async fn holds(scheduler: &Live, id: u64) -> bool {
    for page in 1..=OWNER_PAGES {
        let Ok(value) = answer(scheduler, page, "list", serde_json::json!({"page": page})).await
        else {
            return false;
        };

        let items = value["items"].as_array().into_iter().flatten();

        if items.into_iter().any(|item| item["id"].as_u64() == Some(id)) {
            return true;
        }

        if page >= value["pages"].as_u64().unwrap_or(1) {
            return false;
        }
    }

    false
}

async fn answer(scheduler: &Live, id: u64, method: &str, params: Value) -> Result<Value, String> {
    let response =
        tokio::time::timeout(OWNER_CALL, scheduler.call(Request::call(id, method, params)))
            .await
            .map_err(|_| format!("Scheduler {method} call timed out."))?
            .map_err(|error| error.to_string())?;

    if let Some(error) = response.error {
        return Err(error.message);
    }

    response.result.ok_or_else(|| "Scheduler returned no result.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_pages_name_their_scheduler_and_add_up() {
        let page = |ids: &[u64], total: u64, pages: u64, from: u64| {
            let items = ids.iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>();
            serde_json::json!({"items": items, "total": total, "page": 2, "pages": pages,
                               "from": from, "to": from + ids.len() as u64 - 1})
        };

        let merged = merge(vec![
            ("scheduler".into(), page(&[21, 22], 22, 2, 21)),
            ("cron".into(), page(&[7], 27, 3, 21)),
        ]);

        let owners = merged["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| (item["id"].as_u64().unwrap(), item["scheduler"].as_str().unwrap()))
            .collect::<Vec<_>>();

        assert_eq!(owners, [(21, "scheduler"), (22, "scheduler"), (7, "cron")]);
        assert_eq!(merged["total"], 49);
        assert_eq!(merged["pages"], 3);
        assert_eq!(merged["page"], 2);
        assert_eq!((merged["from"].as_u64(), merged["to"].as_u64()), (Some(41), Some(43)));

        let empty = merge(Vec::new());

        assert_eq!((empty["from"].as_u64(), empty["pages"].as_u64()), (Some(0), Some(1)));
    }
}
