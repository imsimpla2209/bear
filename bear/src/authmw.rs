use std::cell::{Cell};
use std::fmt::Debug;
use std::future::{Ready, ready};

use std::rc::Rc;

use actix_web::dev::{forward_ready, Payload, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::{Error, FromRequest, HttpMessage, HttpRequest, web};
use anyhow::anyhow;
use futures_util::future::LocalBoxFuture;
use crate::db::{DbMain, DbWriteTxn};
use crate::errors::{AnyHandlerError, ApiError};
use crate::interface::{AppContainer, Session};
use crate::oidc::{SESSION_KIND_OIDC};
use crate::utils::Instant;

#[derive(Debug)]
pub struct Authentication {
    pub kind: String,
    pub id: String,
    pub secret: String
}

#[derive(Clone, Debug)]
pub struct PrincipalInner {
    pub auth_kind: String, // use e.g. SESSION_KIND_OIDC
    pub principal: String, // email, device_id, ...
    pub parent: Option<String>, // optional additional data such as shop_id
}

pub struct PrincipalTaken {
    taken: Cell<bool>
}

pub struct AuthMidFactory<AC> {
    db: DbMain,
    required: bool,
    phantom_ac: std::marker::PhantomData<AC>,
}

impl<AC: AppContainer> AuthMidFactory<AC> {
    pub fn new(db: DbMain, required: bool) -> Self {
        Self {
            db,
            required,
            phantom_ac: std::marker::PhantomData,
        }
    }
}

impl <S, B, AC>Transform<S, ServiceRequest> for AuthMidFactory<AC>
    where
        S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
        S::Future: 'static,
        B: 'static,
        AC: AppContainer,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = AuthMiddleware<S, AC>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(AuthMiddleware {
            service: Rc::new(service),
            db: self.db.clone(),
            required: self.required,
            phantom_ac: std::marker::PhantomData
        }))
    }
}

pub struct AuthMiddleware<S, AC: AppContainer> {
    service: Rc<S>,
    db: DbMain,
    required: bool,
    phantom_ac: std::marker::PhantomData<AC>
}

impl<S, B, AC> Service<ServiceRequest> for AuthMiddleware<S, AC>
    where
        S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
        S::Future: 'static,
        B: 'static,
        AC: AppContainer,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {

        async fn use_session<AC: AppContainer>(txn: &mut DbWriteTxn<'_>, sess_found: &AC::S, now: Instant) -> anyhow::Result<()> {
            if sess_found.expires() < now {
                AC::S::delete(txn, &sess_found.code()).await?;
                return Err(ApiError::Expired.into())
            } else {
                AC::S::extend(txn, &sess_found.code(), now + AC::S::lifetime(&sess_found.kind())).await?;
            }
            Ok(())
        }

        // goes from parsed only auth info to verified principal
        async fn verify_auth<AC: AppContainer>(db: &DbMain, objs: web::Data<AC>, auth: Option<Authentication>) -> anyhow::Result<Option<Rc<PrincipalInner>>> {
            if let Some(auth_it) = auth {
                // general case
                let mut txn = db.newtx_write().await?;
                let sess = AC::S::find_session(&mut txn, objs.clone(), &auth_it).await?;
                use_session::<AC>(&mut txn, &sess, objs.utcnow()).await?;
                let _ = txn.commit().await?;
                Ok(Some(Rc::new(sess.as_principal()?)))

            } else {
                Ok(None)
            }
        }

        let service = Rc::clone(&self.service);
        let db = self.db.clone();
        let required = self.required;

        let auth = AC::read_authentication(&req);

        Box::pin(async move {
            let objs = AC::from_request(&req)
                .ok_or(AnyHandlerError::from(ApiError::InvalidState("objs missing".into())))?;
            let principal = verify_auth(&db, objs.clone(),  auth)
                .await
                .map_err(AnyHandlerError::from)?;
            log::debug!("Verified principal {principal:?}");
            if let Some(principal_it) = principal {
                req.extensions_mut().insert(principal_it);
                req.extensions_mut().insert(PrincipalTaken { taken: Cell::new(false) });

            } else if required {
                log::warn!("Principal not found in auth mw");
                return Err(AnyHandlerError::from(ApiError::AuthError1("request.not.authenticated".into())).into());
            }
            let result = service.call(req).await?;

            // check if principal was taken
            if let Some(ptaken) = result.request().extensions().get::<PrincipalTaken>() {
                if !ptaken.taken.get() {
                    // TODO getting this also when handler not found due to path not matching
                    return Err(AnyHandlerError::from(anyhow!("Principal not extracted in a handler under authenticated scope. Is your URL correct?")).into());
                }
            }
            Ok(result)
        })
    }
}

pub fn parse_header(parts: Vec<&str>) -> Option<(String, String)> {
    if parts.len() < 3 { return None; }
    let device_id = parts[ 1 ];
    let token = parts[ 2 ];
    Some((device_id.into(), token.into()))
}

pub fn principal_from_request(req: &HttpRequest, auth_kind: &str) -> Result<Rc<PrincipalInner>, Error> {
    let exts = req.extensions();
    let principal = exts.get::<Rc<PrincipalInner>>()
        .map_or(Err(ApiError::AuthError1("principal.missing".into()).into()), |it|
            if it.auth_kind != auth_kind { Err(ApiError::AuthError1(format!("principal.kind.mismatch:a:{}/e:{}", it.auth_kind, auth_kind))) }
            else { Ok(it.clone()) }
        )
        .map_err(AnyHandlerError::from)
        .map_err(Error::from)?;
    let ptaken = exts.get::<PrincipalTaken>();
    if let Some(ptaken_it) = ptaken {
        ptaken_it.taken.set(true);
    }
    Ok(principal)
}

// todo? I guess previously I worried about too much copying but that's really irrelevant
#[derive(Clone)]
pub struct PrincipalOidc {
    pub email: String
}

impl PrincipalOidc {
    pub fn test_new(email: &str) -> Self {
        PrincipalOidc {
            email: email.into(),
        }
    }
}

impl FromRequest for PrincipalOidc {
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<PrincipalOidc, Error>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let principal = principal_from_request(req, SESSION_KIND_OIDC);

        Box::pin(async move {
            Ok(PrincipalOidc { email: principal?.principal.clone() })
        })
    }
}
