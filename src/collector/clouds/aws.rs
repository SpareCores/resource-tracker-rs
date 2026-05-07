use crate::metrics::CloudInfo;

use super::{imds_get, imds_get_headers, new_imds_agent};

fn fetch_imdsv2_token(agent: &ureq::Agent) -> Option<String> {
    agent
        .put("http://169.254.169.254/latest/api/token")
        .header("X-aws-ec2-metadata-token-ttl-seconds", "21600")
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
    let url = format!("http://169.254.169.254{path}");
    match token {
        Some(t) => imds_get_headers(agent, &url, &[("X-aws-ec2-metadata-token", t)]),
        None => imds_get(agent, &url),
    }
}

const META_ROOT: &str = "http://169.254.169.254/latest/meta-data/";

/// AWS IMDSv2 (token + header) with IMDSv1 fallback. Returns `None` if not on EC2.
///
/// When the IMDSv2 token `PUT` succeeds, the token is valid -- no extra validation `GET`
/// to `META_ROOT` (avoids an unnecessary timeout on non-AWS hosts). If the token
/// fetch fails, fall back to unauthenticated IMDSv1 `GET` on `META_ROOT`.
///
/// After reachability is confirmed the probe additionally verifies
/// `/latest/meta-data/services/domain` equals `"amazonaws.com"` so that other
/// cloud providers exposing an EC2-compatible metadata service are not mistaken
/// for AWS.
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

    // Guard against EC2-compatible metadata services on other clouds.
    let domain = imds_get_with_token(&agent, "/latest/meta-data/services/domain", read_token);
    if domain.as_deref() != Some("amazonaws.com") {
        return None;
    }

    let cloud_region_id =
        imds_get_with_token(&agent, "/latest/meta-data/placement/region", read_token);
    let cloud_zone_id = imds_get_with_token(
        &agent,
        "/latest/meta-data/placement/availability-zone",
        read_token,
    );
    let cloud_instance_type =
        imds_get_with_token(&agent, "/latest/meta-data/instance-type", read_token);
    let cloud_account_id = imds_get_with_token(
        &agent,
        "/latest/meta-data/identity-credentials/ec2/info",
        read_token,
    )
    .and_then(|body| {
        let marker = "\"AccountId\":\"";
        let start = body.find(marker)? + marker.len();
        let end = body[start..].find('"')? + start;
        Some(body[start..end].to_string())
    });

    Some(CloudInfo {
        cloud_vendor_id: Some("aws".to_string()),
        cloud_account_id,
        cloud_region_id,
        cloud_zone_id,
        cloud_instance_type,
    })
}
