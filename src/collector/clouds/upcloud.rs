use crate::metrics::CloudInfo;
use serde::Deserialize;

use super::{imds_get, new_imds_agent};

#[derive(Debug, Deserialize)]
struct Metadata {
    cloud_name: Option<String>,
    region: Option<String>,
}

pub fn probe() -> Option<CloudInfo> {
    let agent = new_imds_agent();
    let body = imds_get(&agent, "http://169.254.169.254/metadata/v1.json")?;
    let meta: Metadata = serde_json::from_str(&body).ok()?;
    if meta.cloud_name.as_deref() != Some("upcloud") {
        return None;
    }

    Some(CloudInfo {
        cloud_vendor_id: Some("upcloud".to_string()),
        cloud_account_id: None,
        cloud_region_id: meta.region.filter(|s| !s.is_empty() && s != "unknown"),
        cloud_zone_id: None,
        cloud_instance_type: None,
    })
}
