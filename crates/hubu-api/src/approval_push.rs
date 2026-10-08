//! Optional private-network, out-of-band approval delivery. No executor calls.
use super::*;
use hmac::{Hmac, Mac};
use std::net::{IpAddr, SocketAddr};

pub(super) const CALLBACK_PATH: &str = "/spend/approval/phone";
const TTL_SECONDS: i64 = 600;

pub(super) struct ApprovalPush {
    publish_url: reqwest::Url,
    callback_url: reqwest::Url,
    topic: String,
    bearer: Option<String>,
    delivered: Mutex<HashMap<String, i64>>,
    resolving: Mutex<()>,
}

impl ApprovalPush {
    pub(super) fn from_env() -> Result<Option<Self>> {
        let Ok(url) = env::var("HUBU_APPROVAL_NTFY_URL") else {
            return Ok(None);
        };
        let topic =
            env::var("HUBU_APPROVAL_NTFY_TOPIC").context("phone approval requires a topic")?;
        let callback = env::var("HUBU_APPROVAL_CALLBACK_URL")
            .context("phone approval requires a private callback URL")?;
        let mut config = Self::new(&url, &callback, &topic)?;
        config.bearer = env::var("HUBU_APPROVAL_NTFY_TOKEN").ok();
        Ok(Some(config))
    }

    pub(super) fn new(url: &str, callback: &str, topic: &str) -> Result<Self> {
        let publish_url = private_url(url)?;
        let callback_url = private_url(callback)?;
        if callback_url.scheme() != "http" {
            return Err(anyhow!("the private phone listener currently supports HTTP only; use an encrypted private network"));
        }
        if callback_url.path() != CALLBACK_PATH {
            return Err(anyhow!("phone callback path must be {CALLBACK_PATH}"));
        }
        if !publish_url.path().eq("/") {
            return Err(anyhow!("ntfy URL must be its private server root"));
        }
        if topic.is_empty()
            || topic.len() > 64
            || !topic
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(anyhow!("invalid private ntfy topic"));
        }
        Ok(Self {
            publish_url,
            callback_url,
            topic: topic.into(),
            bearer: None,
            delivered: Mutex::new(HashMap::new()),
            resolving: Mutex::new(()),
        })
    }

    pub(super) fn callback_bind(&self) -> String {
        format!(
            "{}:{}",
            self.callback_url.host_str().unwrap(),
            self.callback_url.port_or_known_default().unwrap()
        )
    }

    pub(super) fn validate_bind(&self, bind: &str) -> Result<()> {
        let bound: SocketAddr = bind
            .parse()
            .context("phone approval requires a literal private bind address")?;
        validate_peer(bound.ip())?;
        if self.callback_url.host_str() != Some(bound.ip().to_string().as_str())
            || self.callback_url.port_or_known_default() != Some(bound.port())
        {
            return Err(anyhow!(
                "phone callback must match the actual private server bind"
            ));
        }
        Ok(())
    }

    pub(super) fn payload(
        &self,
        decision: &SpendDecisionRecord,
        state: &ServerState,
    ) -> Result<Value> {
        let expires = decision.created_at.timestamp() + TTL_SECONDS;
        let token = signed_token(
            &decision.id.to_string(),
            expires,
            &state.auth.approval_token_hash,
        );
        let agent = state
            .registration
            .lock()
            .map_err(|_| anyhow!("registration lock poisoned"))?
            .agent_for_id(&decision.request.agent_id)?
            .ok_or_else(|| anyhow!("missing approval agent"))?;
        let scope = decision.request.execution_scope.as_ref();
        let description = scope
            .map(|s| {
                format!(
                    "{} · {}",
                    safe_label(&s.provider.display_name),
                    safe_label(&s.capability.display_name)
                )
            })
            .unwrap_or_else(|| "spend request".into());
        let cents = decision.request.amount_cents;
        Ok(json!({
            "topic": self.topic, "title": "Hubu approval",
            "message": format!("{} requests ${}.{:02} · {} · Approve / Deny", safe_label(&agent.display_name), cents / 100, cents % 100, description),
            "actions": (["approve", "deny"].map(|action| json!({
                "action": "http", "label": if action == "approve" {"Approve"} else {"Deny"},
                "url": self.callback_url.as_str(), "method": "POST",
                "headers": {"Content-Type":"application/json"},
                "body": json!({"token": token,"decision":action}).to_string()
            })))
        }))
    }

    pub(super) fn notify(&self, id: &str, state: &ServerState, now: DateTime<Utc>) -> Result<()> {
        let id: SpendDecisionId = id.parse()?;
        let decision = state
            .spend
            .lock()
            .map_err(|_| anyhow!("spend lock poisoned"))?
            .decision_record(&id)
            .ok_or_else(|| anyhow!("unknown decision"))?;
        let expires = decision.created_at.timestamp() + TTL_SECONDS;
        if expires <= now.timestamp()
            || approval_status_for_decision(&decision, state)? != "pending"
        {
            return Ok(());
        }
        let id = id.to_string();
        {
            let mut delivered = self
                .delivered
                .lock()
                .map_err(|_| anyhow!("delivery lock poisoned"))?;
            delivered.retain(|_, expiry| *expiry > now.timestamp());
            if delivered.contains_key(&id) {
                return Ok(());
            }
            delivered.insert(id.clone(), expires);
        }
        // Neither governance nor spend locks are held during network I/O. No redirects/proxies:
        // credentials and approval tokens must never escape to another host.
        let result = (|| {
            let client = reqwest::blocking::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(StdDuration::from_secs(2))
                .build()?;
            let mut request = client
                .post(self.publish_url.clone())
                .json(&self.payload(&decision, state)?);
            if let Some(token) = &self.bearer {
                request = request.bearer_auth(token);
            }
            request.send()?.error_for_status()?;
            Ok(())
        })();
        if result.is_err() {
            self.delivered
                .lock()
                .map_err(|_| anyhow!("delivery lock poisoned"))?
                .remove(&id);
        }
        result
    }
}

fn safe_label(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).take(80).collect()
}

fn private_url(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw).context("invalid private approval URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(anyhow!(
            "approval URL must be HTTP(S) without credentials, query, or fragment"
        ));
    }
    let ip: IpAddr = url
        .host_str()
        .ok_or_else(|| anyhow!("missing private host"))?
        .parse()
        .context("approval URL requires a literal private IPv4 address")?;
    validate_peer(ip)?;
    Ok(url)
}

pub(super) fn validate_peer(ip: IpAddr) -> Result<()> {
    let allowed = match ip {
        IpAddr::V4(v) => {
            v.is_loopback()
                || v.is_private()
                || (v.octets()[0] == 100 && (64..=127).contains(&v.octets()[1]))
        }
        // IPv6 deployment is deliberately excluded until interface/URL handling is qualified.
        IpAddr::V6(_) => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(anyhow!(
            "phone approval requires loopback, RFC1918, or private Tailscale IPv4"
        ))
    }
}

pub(super) fn signed_token(id: &str, expires: i64, key: &str) -> String {
    let data = format!("hubu-phone-v1:{id}:{expires}");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key size");
    mac.update(data.as_bytes());
    let signature = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{id}.{expires}.{signature}")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Action {
    token: String,
    decision: HumanApprovalHttpDecision,
}

pub(super) fn route_callback(request: HttpRequest, state: &ServerState) -> HttpResponse {
    let result = resolve_action(request, state, Utc::now());
    match result {
        Ok(value) => HttpResponse {
            status: 200,
            body: value,
        },
        // Deliberately omit details, ids and capabilities from HTTP errors.
        Err(_) => HttpResponse {
            status: 403,
            body: json!({"error":"invalid, expired, or already resolved phone approval"}),
        },
    }
}

pub(super) fn resolve_action(
    request: HttpRequest,
    state: &ServerState,
    now: DateTime<Utc>,
) -> Result<Value> {
    let push = state
        .approval_push
        .as_ref()
        .ok_or_else(|| anyhow!("phone approval disabled"))?;
    if request.method != "POST"
        || request.headers.contains_key("origin")
        || request
            .headers
            .keys()
            .any(|key| key == "forwarded" || key.starts_with("x-forwarded-"))
    {
        return Err(anyhow!("invalid callback transport"));
    }
    let expected_host = format!(
        "{}:{}",
        push.callback_url.host_str().unwrap(),
        push.callback_url.port_or_known_default().unwrap()
    );
    if request.headers.get("host") != Some(&expected_host)
        || request
            .headers
            .get("content-type")
            .and_then(|v| v.split(';').next())
            != Some("application/json")
    {
        return Err(anyhow!("invalid callback headers"));
    }
    let action: Action = serde_json::from_str(&request.body)?;
    let parts: Vec<_> = action.token.split('.').collect();
    if parts.len() != 3 {
        return Err(anyhow!("invalid token"));
    }
    let expires: i64 = parts[1].parse()?;
    if now.timestamp() >= expires
        || !constant_time_eq(
            action.token.as_bytes(),
            signed_token(parts[0], expires, &state.auth.approval_token_hash).as_bytes(),
        )
    {
        return Err(anyhow!("invalid signature or expiry"));
    }
    let id: SpendDecisionId = parts[0].parse()?;
    // Serialize competing Approve/Deny callbacks. The durable decision is the consumption
    // record, so restarting the server never makes an old token usable again.
    let _guard = push
        .resolving
        .lock()
        .map_err(|_| anyhow!("approval lock poisoned"))?;
    let decision = state
        .spend
        .lock()
        .map_err(|_| anyhow!("spend lock poisoned"))?
        .decision_record(&id)
        .ok_or_else(|| anyhow!("unknown decision"))?;
    if expires != decision.created_at.timestamp() + TTL_SECONDS
        || approval_status_for_decision(&decision, state)? != "pending"
    {
        return Err(anyhow!("already consumed"));
    }
    let response = resolve_spend_approval_at(json!({"approval_request_id":parts[0], "decision":match action.decision {HumanApprovalHttpDecision::Approve=>"approve",HumanApprovalHttpDecision::Deny=>"deny"}}).to_string(), state, now)?;
    // Approval reserves budget only. Never hand the phone an executor authorization token.
    Ok(
        json!({"status":response.approval.map(|approval| approval.status),"message":"Decision recorded in Hubu. Resume the same operation handle."}),
    )
}
