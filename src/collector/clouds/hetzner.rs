use crate::metrics::CloudInfo;

use super::{imds_get, new_imds_agent};

pub fn probe() -> Option<CloudInfo> {
    let agent = new_imds_agent();
    let text = imds_get(&agent, "http://169.254.169.254/hetzner/v1/metadata")?;

    let mut instance_type: Option<String> = None;
    let mut region: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "instance-id" => instance_type = Some(value.trim().to_string()),
            "region" => region = Some(value.trim().to_string()),
            _ => {}
        }
    }

    Some(CloudInfo {
        cloud_vendor_id: Some("hcloud".to_string()),
        cloud_account_id: None,
        cloud_region_id: region.filter(|s| !s.is_empty() && s != "unknown"),
        cloud_zone_id: None,
        cloud_instance_type: instance_type.filter(|s| !s.is_empty() && s != "unknown"),
    })
}
