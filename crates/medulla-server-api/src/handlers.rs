//! Handler modules that live below `medulla-server` (graph-learning ops, which the
//! write-back replay arm in `mutation_apply` re-runs) and the method-handler
//! registry that every served method is dispatched through
//! (MEDULLA-CONTRACT-R056.5).

use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Response};
use tokio::sync::RwLock;

use crate::authority_context::VerifiedRequestContext;
use crate::state::ServerState;

/// The request fields every handler needs after `req.method` has been moved out
/// of the `Request`.
pub struct DispatchHeader {
    /// Request id echoed in the response.
    pub id: u64,
    /// Target graph name.
    pub graph: String,
    /// Calling agent, when one is named.
    pub agent_id: Option<String>,
}

/// The authenticated, preamble-resolved context shared by every handler.
/// All fields are shared references or flags, so this is `Copy`.
#[derive(Clone, Copy)]
pub struct DispatchCtx<'a> {
    /// The shared server state.
    pub state: &'a Arc<RwLock<ServerState>>,
    /// The request header.
    pub req: &'a DispatchHeader,
    /// The verified request context.
    pub verified_context: &'a VerifiedRequestContext,
    /// The request already carries state-machine authority.
    pub state_machine_authorized: bool,
    /// The request is the identity bootstrap.
    pub identity_bootstrap: bool,
}

/// The future a handler returns: `Break` when it served the method, `Continue`
/// handing the method back unclaimed.
pub type HandlerFuture<'a> =
    Pin<Box<dyn std::future::Future<Output = ControlFlow<Response, Method>> + Send + 'a>>;

/// One domain's entry into the served-method dispatch.
pub trait MethodHandler: Send + Sync {
    /// Stable name of the handler, for diagnostics and the registry order test.
    fn name(&self) -> &'static str;

    /// Serve `method` or hand it back as `ControlFlow::Continue`.
    fn try_handle<'a>(&'a self, ctx: DispatchCtx<'a>, method: Method) -> HandlerFuture<'a>;
}

/// The ordered list of method handlers. Dispatch walks it front to back and
/// stops at the first handler that serves the method.
#[derive(Default)]
pub struct HandlerRegistry {
    handlers: Vec<Box<dyn MethodHandler>>,
}

impl HandlerRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a handler; it runs after every handler registered before it.
    pub fn register(&mut self, handler: impl MethodHandler + 'static) {
        self.handlers.push(Box::new(handler));
    }

    /// The handler names in dispatch order.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.handlers.iter().map(|handler| handler.name()).collect()
    }

    /// Walk the handlers in order. `Continue` carries a method no handler claimed.
    pub async fn dispatch(
        &self,
        ctx: DispatchCtx<'_>,
        method: Method,
    ) -> ControlFlow<Response, Method> {
        let mut method = method;
        for handler in &self.handlers {
            method = handler.try_handle(ctx, method).await?;
        }
        ControlFlow::Continue(method)
    }
}

#[cfg(feature = "graphlearn")]
pub mod graphlearn;
