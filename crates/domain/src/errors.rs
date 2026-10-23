use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    pub correlation_id: String,
    pub retryable: bool,
    pub fields: BTreeMap<String, Value>,
}

impl Problem {
    pub fn new(
        code: impl Into<String>,
        title: impl Into<String>,
        status: u16,
        correlation_id: impl Into<String>,
    ) -> Self {
        let code = code.into();
        Self {
            problem_type: format!("https://buglogin.local/problems/{code}"),
            title: title.into(),
            status,
            code,
            detail: None,
            instance: None,
            correlation_id: correlation_id.into(),
            retryable: false,
            fields: BTreeMap::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_field(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(name.into(), value.into());
        self
    }

    pub fn invalid_transition(
        resource: impl Into<String>,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Self {
        let resource = resource.into();
        let from = from.into();
        let to = to.into();
        Self::new(
            "invalid_state_transition",
            "Invalid state transition",
            409,
            "domain",
        )
        .with_detail(format!("Cannot transition {resource} from {from} to {to}"))
        .with_field("resource", resource)
        .with_field("from", from)
        .with_field("to", to)
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.title, self.code)
    }
}

impl std::error::Error for Problem {}
