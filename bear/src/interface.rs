use std::fmt::Debug;
use actix_web::dev::ServiceRequest;
use actix_web::web;
use async_trait::async_trait;
use crate::authmw::{Authentication, PrincipalInner};
use crate::db::DbTxn;
use crate::utils::Instant;

pub enum CommonSecretKind {
    OidcSecret,
}

pub trait AppContainer : Send + Sync {
    type S: Session;
    type Cfg: crate::cfg::Cfg;

    fn cfg(&self) -> &Self::Cfg;

    fn utcnow(&self) -> Instant;
    fn from_request(req: &ServiceRequest) -> Option<&web::Data<Self>>;
    fn read_authentication(req: &ServiceRequest) -> Option<Authentication>;
    fn secret(&self, kind: CommonSecretKind) -> &str;
}

#[async_trait]
pub trait Session : Sized + Send + Debug {
    fn code(&self) -> &str;
    fn expires(&self) -> Instant;
    fn kind(&self) -> String;
    fn email(&self) -> Option<&str>;

    async fn find_session<AC: AppContainer>(db: &mut DbTxn<'_>, objs: web::Data<AC>, auth: &Authentication) -> anyhow::Result<Self>;
    async fn extend(db: &mut DbTxn<'_>, code: &str, expires: Instant) -> anyhow::Result<()>;
    async fn delete(db: &mut DbTxn<'_>, code: &str) -> anyhow::Result<()>;
    async fn insert(db: &mut DbTxn<'_>, s: &Self) -> anyhow::Result<()>;

    fn new_oidc(expires: Instant, email: String) -> Self;
    fn lifetime(kind: &str) -> i64;

    fn as_principal(&self) -> anyhow::Result<PrincipalInner>;
}