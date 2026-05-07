use crate::metrics::CloudInfo;

use super::{imds_get, imds_get_headers, new_imds_agent};

fn fetch_imdsv2_token(agent: &ureq::Agent) -> Option<String> {
    agent
        .put("http://100.100.100.200/latest/api/token")
        .header("X-aliyun-ecs-metadata-token-ttl-seconds", "21600")
        .send_empty()
        .ok()
        .and_then(|mut r| {
            if !r.status().is_success() {
                return None;
            }
            r.body_mut().read_to_string().ok()
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn imds_get_with_token(agent: &ureq::Agent, path: &str, token: Option<&str>) -> Option<String> {
    let url = format!("http://100.100.100.200{path}");
    match token {
        Some(t) => imds_get_headers(agent, &url, &[("X-aliyun-ecs-metadata-token", t)]),
        None => imds_get(agent, &url),
    }
}

const META_ROOT: &str = "http://100.100.100.200/latest/meta-data/";

/// Alibaba Cloud ECS IMDSv2 (token + header) with IMDSv1 fallback.
///
/// Uses `100.100.100.200`, the Alibaba-specific link-local address, as the
/// confirmation signal. Returns `None` if neither the token PUT nor a plain
/// GET on the metadata root succeeds.
///
/// Reference: <https://www.alibabacloud.com/help/en/ecs/user-guide/view-instance-metadata/>
pub fn probe() -> Option<CloudInfo> {
    let agent = new_imds_agent();
    let token = fetch_imdsv2_token(&agent);

    let read_token: Option<&str> = if token.is_some() {
        token.as_deref()
    } else if imds_get(&agent, META_ROOT).is_some() {
        None
    } else {
        return None;
    };

    let cloud_instance_type = imds_get_with_token(
        &agent,
        "/latest/meta-data/instance/instance-type",
        read_token,
    );
    let cloud_region_id = imds_get_with_token(&agent, "/latest/meta-data/region-id", read_token);

    Some(CloudInfo {
        cloud_vendor_id: Some("alicloud".to_string()),
        cloud_account_id: None,
        cloud_region_id: cloud_region_id.filter(|s| !s.is_empty() && s != "unknown"),
        cloud_zone_id: None,
        cloud_instance_type: cloud_instance_type.filter(|s| !s.is_empty() && s != "unknown"),
    })
}
