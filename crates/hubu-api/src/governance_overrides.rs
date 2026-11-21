//! Strict opt-in delegation for two monotonic governance changes.
use super::*;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum GovernanceOverride {
    LowerBudget,
    AddDenyRule,
}

impl GovernanceOverride {
    const ALL: [Self; 2] = [Self::LowerBudget, Self::AddDenyRule];

    pub(super) fn key(self) -> &'static str {
        match self {
            Self::LowerBudget => "lower_budget",
            Self::AddDenyRule => "add_deny_rule",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::LowerBudget => "Lower an existing budget",
            Self::AddDenyRule => "Append a deny rule",
        }
    }

    fn permission(self) -> &'static str {
        match self {
            Self::LowerBudget => "lower existing budgets",
            Self::AddDenyRule => "append deny rules to existing assigned policies",
        }
    }

    fn risk(self) -> &'static str {
        match self {
            Self::LowerBudget => "A prompt-injected agent could cut spending limits and interrupt work.",
            Self::AddDenyRule => "A prompt-injected agent could block legitimate spend for every agent using a policy.",
        }
    }

    pub(super) fn action(self) -> &'static str {
        match self {
            Self::LowerBudget => "budget.version.create",
            Self::AddDenyRule => "policy.apply",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SetOverride {
    action: GovernanceOverride,
    enabled: bool,
    #[serde(default)]
    acknowledge_warning: bool,
}

impl SetOverride {
    pub(super) fn parse(body: &str) -> Result<Self> {
        let change: Self = serde_json::from_str(body)?;
        if change.enabled && !change.acknowledge_warning {
            return Err(anyhow!(
                "acknowledge the warning before enabling an override"
            ));
        }
        Ok(change)
    }

    pub(super) fn key(&self) -> &'static str {
        self.action.key()
    }

    pub(super) fn reason(&self) -> String {
        let permission = self.action.permission();
        if self.enabled {
            format!(
                "allow agents to {permission} without you. {}",
                self.action.risk().trim_end_matches('.')
            )
        } else {
            format!("remove permission for agents to {permission}")
        }
    }
}

fn strict_permissions() -> Vec<String> {
    let mut permissions = Vec::new();
    for &(method, path, access) in ROUTE_ACCESS {
        if access != RouteAccess::Agent {
            continue;
        }
        let label = match (method, path) {
            ("POST", "/spend/authorize" | "/spend") => "Request authorization and submit spend",
            ("POST", "/spend/void") => "Void unclaimed spend authorizations",
            ("GET", "/spend/approval") | ("POST", "/spend/approval/prompt") => {
                "Check spend approvals and prompt the owner to decide"
            }
            ("GET", "/budgets" | "/watch") => "View budgets and watch for changes",
            ("GET", "/ledger" | "/ledger/transactions" | "/ledger/summary") => {
                "View the ledger and spend summaries"
            }
            ("GET", "/spend/authorizations" | "/spend/authorizations/show") => {
                "View spend authorization records"
            }
            ("GET", "/agents" | "/user" | "/owner/presence") => {
                "View agents, the current user, and owner confirmation availability"
            }
            ("GET", "/policies" | "/policies/show") => "View policies",
            ("POST", "/agents/register") => "Register agents",
            _ => {
                permissions.push(format!("{method} {path}"));
                continue;
            }
        };
        if !permissions.iter().any(|permission| permission == label) {
            permissions.push(label.to_string());
        }
    }
    permissions
}

pub(super) fn list(state: &ServerState) -> Result<Value> {
    let enabled = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .enabled_governance_overrides()?;
    let overrides: Vec<_> = GovernanceOverride::ALL
        .into_iter()
        .map(|action| {
            json!({
                "action": action.key(),
                "label": action.label(),
                "permission": format!("Agents can {} without your confirmation.", action.permission()),
                "risk": action.risk(),
                "enabled": enabled.iter().any(|key| key == action.key()),
            })
        })
        .collect();
    Ok(json!({ "strict_permissions": strict_permissions(), "overrides": overrides }))
}

pub(super) fn set(body: &str, state: &ServerState) -> Result<Value> {
    let change = SetOverride::parse(body)?;
    state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .set_governance_override(change.key(), change.enabled)?;
    Ok(json!({ "action": change.key(), "enabled": change.enabled }))
}

/// Called under the dispatch lock, which must remain held through the write.
/// A route and its body must both qualify. The normal handler still validates
/// the write and uses compare-and-set against the revision we check here.
pub(super) fn delegation(
    request: &HttpRequest,
    state: &ServerState,
) -> Result<Option<(GovernanceOverride, String)>> {
    if request.method != "POST" {
        return Ok(None);
    }
    let budget_id = budget_versions_public_id(&request.path);
    let action = match (budget_id, request.path.as_str()) {
        (Some(_), _) => GovernanceOverride::LowerBudget,
        (None, "/policies") => GovernanceOverride::AddDenyRule,
        _ => return Ok(None),
    };
    let enabled = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?
        .governance_override_enabled(action.key())?;
    if !enabled {
        return Ok(None);
    }
    let target = match budget_id {
        Some(id) => lower_budget(id, &request.body, state)?,
        None => append_deny_rule(&request.body, state)?,
    };
    Ok(target.map(|target| (action, target)))
}

fn lower_budget(id: &str, body: &str, state: &ServerState) -> Result<Option<String>> {
    let Ok(request) = parse_update_budget_limit_request(body) else {
        return Ok(None);
    };
    let Ok(amount) = decimal_amount(
        request.amount_limit.as_deref(),
        "amount_limit",
        "budget update",
    ) else {
        return Ok(None);
    };
    let user = authenticated_user_context(state)?;
    let revision = u64::try_from(request.expected_revision).ok();
    Ok(budgets_for_user(&user, state)?
        .into_iter()
        .find(|entry| public_budget_id(&entry.budget.id) == id)
        .filter(|entry| {
            revision == Some(entry.version.revision)
                && amount > Amount::ZERO
                && amount < entry.version.amount_limit
        })
        .map(|_| id.to_string()))
}

fn append_deny_rule(body: &str, state: &ServerState) -> Result<Option<String>> {
    let Ok(request) = serde_json::from_str::<AddPolicyHttpRequest>(body) else {
        return Ok(None);
    };
    let Some(yaml) = request.policy_yaml.as_deref() else {
        return Ok(None);
    };
    // Keep the handler's normal API provenance for delegated writes.
    if request.daily_limit.is_some() || request.source.is_some() {
        return Ok(None);
    }
    let Ok(mut proposed) = Policy::from_yaml_str(yaml) else {
        return Ok(None);
    };
    let user = authenticated_user_context(state)?;
    proposed.owner_user_id = user.user_id.clone();
    if validate_policy(&proposed).is_err() {
        return Ok(None);
    }
    let Some(rule) = proposed.rules.pop() else {
        return Ok(None);
    };
    if rule.effect != Effect::Deny || proposed.rules.iter().any(|existing| existing.id == rule.id) {
        return Ok(None);
    }
    let scope = match resolve_policy_scope(request.agent_id.as_deref(), &user, state) {
        Ok(scope) => scope,
        Err(_) => return Ok(None),
    };
    let governance = state
        .governance
        .lock()
        .map_err(|_| anyhow!("governance store lock poisoned"))?;
    let Some(assignment) = governance
        .load_policy_assignments()?
        .into_iter()
        .find(|assignment| assignment.owner_user_id == user.user_id && assignment.scope == scope)
    else {
        return Ok(None);
    };
    let resource = governance
        .load_policy_by_id(&user.user_id, &assignment.policy_id)?
        .ok_or_else(|| anyhow!("assigned policy resource is missing"))?;

    // The apply handler must update the same resource and keep its assignment
    // and metadata. Appending a deny preserves or tightens every decision
    // because a matching deny has highest precedence.
    let same_resource = request.declarative_key.as_deref().unwrap_or(&proposed.id);
    let same_name = request.display_name.as_deref().unwrap_or(same_resource);
    if same_resource.trim() == resource.declarative_key
        && same_name.trim() == resource.display_name
        && request.expected_revision == Some(resource.current_revision)
        && request.expected_hash.as_deref() == Some(resource.current_hash.as_str())
        && serde_json::to_value(&proposed)? == serde_json::to_value(&resource.policy)?
    {
        return Ok(Some(resource.policy_id));
    }
    Ok(None)
}
