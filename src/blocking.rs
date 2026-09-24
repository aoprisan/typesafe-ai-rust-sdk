//! Synchronous client (feature `blocking`). Each client owns a private current-thread Tokio
//! runtime, shared with its clones; do not call it from inside an async context.

use std::sync::Arc;
use std::time::Duration;

use http::header::{HeaderName, HeaderValue};
use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::question::Questions;
use crate::response::{ListModelsResponse, SystemOneResponse};
use crate::retry::RetryPolicy;
use crate::rubric::Rubric;

/// Blocking TypeSafe client. Cheap to clone; clones share the runtime and the connection pool.
#[derive(Debug, Clone)]
pub struct Client {
    inner: crate::Client,
    rt: Arc<tokio::runtime::Runtime>,
}

impl Client {
    /// Wrap an async client configured via [`crate::Client::builder`] (or build one with
    /// [`ClientBuilder::build_blocking`](crate::ClientBuilder::build_blocking)).
    ///
    /// # Errors
    ///
    /// [`Error::Config`] if the runtime cannot be started.
    pub fn new(inner: crate::Client) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Config(format!("could not start runtime: {e}")))?;
        Ok(Self {
            inner,
            rt: Arc::new(rt),
        })
    }

    /// A client configured entirely from the environment.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] as for [`crate::Client::from_env`], or if the runtime cannot be started.
    pub fn from_env() -> Result<Self> {
        Self::new(crate::Client::from_env()?)
    }

    /// The default model.
    pub fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    /// The Models resource; see [`crate::Client::models`].
    pub fn models(&self) -> Models<'_> {
        Models { client: self }
    }

    /// See [`crate::Client::system_one`].
    pub fn system_one<S: Serialize>(
        &self,
        state: S,
        questions: impl Into<Questions>,
    ) -> SystemOneRequest<'_> {
        SystemOneRequest {
            rt: &self.rt,
            req: self.inner.system_one(state, questions),
        }
    }

    /// See [`crate::Client::ask`].
    pub fn ask<R: Rubric>(&self, state: impl Serialize) -> AskRequest<'_, R> {
        AskRequest {
            rt: &self.rt,
            req: self.inner.ask(state),
        }
    }

    /// `GET /v1/models`.
    #[deprecated(note = "use `client.models().list()`, as on the async client")]
    pub fn list_models(&self) -> ListModelsRequest<'_> {
        self.models().list()
    }
}

/// The blocking Models resource.
#[derive(Debug, Clone, Copy)]
pub struct Models<'a> {
    client: &'a Client,
}

impl<'a> Models<'a> {
    /// `GET /v1/models`.
    pub fn list(&self) -> ListModelsRequest<'a> {
        ListModelsRequest {
            rt: &self.client.rt,
            req: self.client.inner.models().list(),
        }
    }
}

macro_rules! forward {
    ($($name:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {$(
        #[doc = concat!("See the async request's `", stringify!($name), "`.")]
        pub fn $name(mut self, $($arg: $ty),*) -> Self {
            self.req = self.req.$name($($arg),*);
            self
        }
    )*};
}

/// Blocking `POST /v1/systemone`.
#[must_use = "call .send()"]
#[derive(Debug)]
pub struct SystemOneRequest<'a> {
    rt: &'a tokio::runtime::Runtime,
    req: crate::SystemOneRequest,
}

impl SystemOneRequest<'_> {
    forward!(
        retry(policy: RetryPolicy),
        timeout(timeout: Duration),
        header(name: HeaderName, value: HeaderValue),
    );

    /// Override the model for this call.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.req = self.req.model(model);
        self
    }

    /// Add a top-level body field.
    pub fn extra_body(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.req = self.req.extra_body(key, value);
        self
    }

    /// Send and wait.
    ///
    /// # Errors
    ///
    /// As for the async request's `send`.
    ///
    /// # Panics
    ///
    /// When called from inside an async runtime.
    pub fn send(self) -> Result<SystemOneResponse> {
        self.rt.block_on(self.req.send())
    }
}

/// Blocking [`crate::Client::ask`].
#[must_use = "call .send()"]
#[derive(Debug)]
pub struct AskRequest<'a, R> {
    rt: &'a tokio::runtime::Runtime,
    req: crate::AskRequest<R>,
}

impl<R: Rubric> AskRequest<'_, R> {
    forward!(
        retry(policy: RetryPolicy),
        timeout(timeout: Duration),
        header(name: HeaderName, value: HeaderValue),
    );

    /// Override the model for this call.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.req = self.req.model(model);
        self
    }

    /// Add a top-level body field.
    pub fn extra_body(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.req = self.req.extra_body(key, value);
        self
    }

    /// Send, wait, and decode the answers.
    ///
    /// # Errors
    ///
    /// As for the async request's `send`.
    ///
    /// # Panics
    ///
    /// When called from inside an async runtime.
    pub fn send(self) -> Result<R> {
        self.rt.block_on(self.req.send())
    }
}

/// Blocking `GET /v1/models`.
#[must_use = "call .send()"]
#[derive(Debug)]
pub struct ListModelsRequest<'a> {
    rt: &'a tokio::runtime::Runtime,
    req: crate::ListModelsRequest,
}

impl ListModelsRequest<'_> {
    forward!(
        retry(policy: RetryPolicy),
        timeout(timeout: Duration),
        header(name: HeaderName, value: HeaderValue),
    );

    /// Send and wait.
    ///
    /// # Errors
    ///
    /// As for the async request's `send`.
    ///
    /// # Panics
    ///
    /// When called from inside an async runtime.
    pub fn send(self) -> Result<ListModelsResponse> {
        self.rt.block_on(self.req.send())
    }
}
