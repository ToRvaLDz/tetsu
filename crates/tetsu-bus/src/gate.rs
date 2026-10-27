//! The capability gate of the facade (#28): a handler wrapper that decides
//! every call before the server sees it.
//!
//! [`Gated`] runs [`access::route`](crate::access::route) on each call and
//! then does one of four things:
//!
//! - **pass** the call to the inner handler;
//! - **refuse** it with `<interface>.PermissionDenied`;
//! - **ask `tetsu-capd`** (`org.tetsu.cap.v1.Check`, at the fixed path of the
//!   user, after checking who serves it) and run the call only on `allow`;
//! - **ask the [`Authority`]** (polkit, in a later slice) and run the call
//!   only on a yes.
//!
//! The two asks answer with [`Response::Deferred`], so the connection keeps
//! being read while they wait: a caller that goes away drops the `Check`
//! connection, and capd withdraws its ask. Every error on the way refuses:
//! connect, server check, remote error, malformed reply, end of file. Each
//! refusal writes one line to the log.

use std::rc::Rc;

use serde_json::json;
use tokio::sync::oneshot;

use crate::access::{
    EtcFiles, ExpectedServer, Groups, MethodKind, Route, Scope, capd_path, log_line, method_kind,
    permission_denied, route,
};
use crate::cap::{CheckParams, CheckPeer, Client, Decision};
use crate::endpoint::Endpoint;
use crate::peer::Peer;
use crate::server::{Handler, Request, Response};

/// The unit that must serve the capability service of a user.
const CAPD_UNIT: &str = "tetsu-capd.service";

/// Decides a call from a peer that is not the agent, on a system service.
pub trait Authority: 'static {
    /// Whether the call may run. Anything unclear is `false`.
    fn authorize(&self, peer: &Peer, method: &str) -> impl Future<Output = bool>;
}

/// Root, when it is not the agent: the policy of the system services until
/// polkit is wired in.
#[derive(Debug, Clone, Copy, Default)]
pub struct RootOnly;

impl Authority for RootOnly {
    async fn authorize(&self, peer: &Peer, _method: &str) -> bool {
        peer.uid == 0 && !peer.agent
    }
}

/// Everything the gate depends on, so a test can replace each part.
pub struct GateConfig<G, A> {
    /// Session or system service.
    pub scope: Scope,
    /// The owner of a session service.
    pub own_uid: u32,
    /// Where the capability service of a user is, and who must serve it.
    pub capd: Box<dyn Fn(u32) -> Endpoint>,
    /// Who is in `wheel`.
    pub groups: G,
    /// Who decides for a peer that is not the agent, on a system service.
    pub authority: A,
    /// Where a refusal is written.
    pub log: Box<dyn Fn(&str)>,
}

/// A handler behind the gate.
pub struct Gated<H, G, A> {
    inner: Rc<H>,
    config: Rc<GateConfig<G, A>>,
}

impl<H, G, A> Gated<H, G, A> {
    /// A gate with its parts given.
    pub fn with(inner: H, config: GateConfig<G, A>) -> Self {
        Self {
            inner: Rc::new(inner),
            config: Rc::new(config),
        }
    }
}

fn production(scope: Scope) -> GateConfig<EtcFiles, RootOnly> {
    GateConfig {
        scope,
        own_uid: rustix::process::geteuid().as_raw(),
        capd: Box::new(|uid| {
            let expected = ExpectedServer::UserUnit {
                uid,
                unit: CAPD_UNIT.to_owned(),
            };
            Endpoint::new(capd_path(uid), expected)
        }),
        groups: EtcFiles::new("/"),
        authority: RootOnly,
        log: Box::new(|line| eprintln!("{line}")),
    }
}

impl<H> Gated<H, EtcFiles, RootOnly> {
    /// The gate of a per-user service: only its owner and root may call,
    /// and the agent's gated calls go to the capd of the owner.
    pub fn session(inner: H) -> Self {
        Self::with(inner, production(Scope::Session))
    }
}

impl<H, A: Authority> Gated<H, EtcFiles, A> {
    /// The gate of a system service. An agent in `wheel` goes to its own
    /// capd, and an agent outside `wheel` is refused; anyone else goes to
    /// `authority`.
    pub fn system(inner: H, authority: A) -> Self {
        let GateConfig {
            scope,
            own_uid,
            capd,
            groups,
            log,
            ..
        } = production(Scope::System);
        let config = GateConfig {
            scope,
            own_uid,
            capd,
            groups,
            authority,
            log,
        };
        Self::with(inner, config)
    }
}

/// Who answers an ask.
#[derive(Clone, Copy)]
enum Ask {
    Capd(u32),
    Authority,
}

impl<H: Handler, G: Groups + 'static, A: Authority> Handler for Gated<H, G, A> {
    async fn call(&self, peer: Rc<Peer>, request: Request) -> Response {
        let config = &self.config;
        let kind = method_kind(&request.method);
        // Only an agent on a system service needs `wheel`, and only for a
        // gated call.
        let in_wheel = config.scope == Scope::System
            && kind == MethodKind::Gated
            && peer.agent
            && config.groups.in_wheel(peer.uid);
        match route(config.scope, &peer, kind, config.own_uid, in_wheel) {
            Route::Pass => self.inner.call(peer, request).await,
            Route::Deny => {
                let why = match config.scope {
                    Scope::Session => "a peer of another user",
                    Scope::System => "an agent outside wheel",
                };
                refuse(config, &request.method, &peer, why)
            }
            Route::AskCapd { uid } => self.defer(peer, request, Ask::Capd(uid)),
            Route::AskAuthority => self.defer(peer, request, Ask::Authority),
        }
    }
}

impl<H: Handler, G: Groups + 'static, A: Authority> Gated<H, G, A> {
    /// Decides in a task of its own, so the connection keeps being read.
    /// The task ends when the caller goes away, and drops everything it
    /// holds, the `Check` connection included.
    fn defer(&self, peer: Rc<Peer>, request: Request, ask: Ask) -> Response {
        let (mut tx, rx) = oneshot::channel();
        let inner = self.inner.clone();
        let config = self.config.clone();
        tokio::task::spawn_local(async move {
            let work = async {
                match decide(&config, ask, &peer, &request).await {
                    Ok(()) => settle(inner.call(peer.clone(), request.clone()).await).await,
                    Err(why) => Some(refuse(&config, &request.method, &peer, &why)),
                }
            };
            tokio::select! {
                () = tx.closed() => {}
                response = work => {
                    // Without a response the sender is dropped, and the
                    // connection with it.
                    if let Some(response) = response {
                        let _ = tx.send(response);
                    }
                }
            }
        });
        Response::Deferred(rx)
    }
}

/// A server that itself answers later is waited for here: the facade does
/// not follow a `Deferred` of a `Deferred`.
async fn settle(mut response: Response) -> Option<Response> {
    while let Response::Deferred(rx) = response {
        response = rx.await.ok()?;
    }
    Some(response)
}

fn refuse<G, A>(config: &GateConfig<G, A>, method: &str, peer: &Peer, why: &str) -> Response {
    (config.log)(&log_line(method, peer, why));
    Response::Error(permission_denied(method))
}

/// `Ok` to run the call; `Err` is the reason it is refused.
async fn decide<G, A: Authority>(
    config: &GateConfig<G, A>,
    ask: Ask,
    peer: &Peer,
    request: &Request,
) -> Result<(), String> {
    match ask {
        Ask::Capd(uid) => check(config, uid, peer, request).await,
        Ask::Authority => {
            if config.authority.authorize(peer, &request.method).await {
                Ok(())
            } else {
                Err("the authority said no".to_owned())
            }
        }
    }
}

/// `org.tetsu.cap.v1.Check` at the capd of `uid`. Only a well-formed `allow`
/// is `Ok`.
async fn check<G, A>(
    config: &GateConfig<G, A>,
    uid: u32,
    peer: &Peer,
    request: &Request,
) -> Result<(), String> {
    let endpoint = (config.capd)(uid);
    let mut client = Client::connect_to(&endpoint)
        .await
        .map_err(|e| format!("capd is not available: {e}"))?;
    // The IDL wants an object.
    let args = if request.parameters.is_null() {
        json!({})
    } else {
        request.parameters.clone()
    };
    let params = CheckParams {
        peer: CheckPeer {
            pid: i64::from(peer.pid),
            uid: i64::from(peer.uid),
            unit: peer.sender_name(),
        },
        method: request.method.clone(),
        args,
    };
    let reply = client
        .check(&params)
        .await
        .map_err(|e| format!("the check failed: {e}"))?;
    match reply.decision {
        Decision::Allow => Ok(()),
        Decision::Deny => Err(match reply.denied_by {
            Some(by) => format!("capd denied ({by:?})"),
            None => "capd denied".to_owned(),
        }),
    }
}
