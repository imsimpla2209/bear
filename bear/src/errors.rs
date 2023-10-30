use std::fmt::{Debug, Display, Formatter};
use std::future::{Ready, ready};
use std::rc::Rc;
use actix_web::dev::{forward_ready, Payload, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::{Error, FromRequest, HttpMessage, HttpRequest, web};
use anyhow::anyhow;
use futures_util::future::LocalBoxFuture;
use actix_web::body::BoxBody;
use actix_web::{HttpResponse, HttpResponseBuilder};


// points into locales
pub type ErrorMessage = String;

// TODO remove EARN specific
// TODO distinguish between user-got-it-wrong and wait-why-its-not-working errors (for monitoring)
// errors that are disclosed to client
#[derive(PartialEq, thiserror::Error, Debug, Clone)]
pub enum ApiError {
    #[error("WebhookAuthentication")]
    WebhookAuthentication,
    #[error("CryptoError")]
    CryptoError,
    #[error("UnknownStep({0})")]
    UnknownStep(String),
    #[error("InvalidStep Definition for ({0})")]
    InvalidStepDef(String),
    #[error("TooLarge")]
    TooLarge,
    #[error("NotFound({0})")]
    NotFound(String),
    #[error("LockError")]
    LockError,
    #[error("InvalidInput")]
    InvalidInput,
    #[error("AuthError")]
    AuthError,
    #[error("AuthError1({0})")]
    AuthError1(String), // create string based error ids to be more specific
    #[error("MissingVar")]
    MissingVar,
    #[error("InvalidState({0})")]
    InvalidState(String),
    #[error("InsufficientEarn")]
    InsufficientEarn,
    #[error("InsufficientLvl")]
    InsufficientLvl,
    #[error("Expired")]
    Expired,
    #[error("UserError({0})")]
    UserError(ErrorMessage), // used to wrap user-facing error message in chatbot

    // convention: subject.property, for example: user.id.missing
    #[error("{0}")]
    Any(String), // create string based error ids to be more specific
    #[error("{0}:{1}")]
    Any1(String, String),
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Disabled")]
    Disabled,
}

pub fn map_os_err<R, T : Debug>(v: Result<R, T>) -> std::io::Result<R> {
    v.map_err(|e| {
        let estr = format!("{:?}", e);
        log::error!("Error when starting:[{:?}", estr.clone());
        std::io::Error::new(std::io::ErrorKind::Other, estr)
    })
}

pub fn anyerr<T>(code: &str) -> Result<T, anyhow::Error> {
    Err(ApiError::Any(String::from(code)).into())
}

pub fn anyerr_in(code: &str) -> anyhow::Error {
    ApiError::Any(String::from(code)).into()
}

pub struct AnyHandlerError(anyhow::Error);

impl Debug for AnyHandlerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl Display for AnyHandlerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

// don't implement generic From<E> because it conflicts
/*
impl<E> From<E> for AnyHandlerError
    where E: Error + Send + Sync + 'static
{
    fn from(e: E) -> Self {
        AnyHandlerError(e.into())
    }
}
 */

impl From<anyhow::Error> for AnyHandlerError
{
    fn from(e: anyhow::Error) -> Self {
        AnyHandlerError(e.into())
    }
}

impl From<ApiError> for AnyHandlerError {
    fn from(e: ApiError) -> Self {
        Self(e.into())
    }
}

impl From<sqlx::Error> for AnyHandlerError {
    fn from(e: sqlx::Error) -> Self {
        Self(e.into())
    }
}

impl From<std::io::Error> for AnyHandlerError {
    fn from(e: std::io::Error) -> Self {
        Self(e.into())
    }
}

impl actix_web::ResponseError for AnyHandlerError {
    fn error_response(&self) -> HttpResponse<BoxBody> {
        log::error!("Web handler error:[{:?}", self);
        let (errstr, code) = match self.0.downcast_ref::<ApiError>() {
            Some(t @ ApiError::AuthError) | Some(t @ ApiError::Expired) | Some(t @ ApiError::AuthError1(_)) =>
                (t.to_string(), http::status::StatusCode::UNAUTHORIZED),
            Some(t) =>
                (t.to_string(), http::status::StatusCode::BAD_REQUEST),
            None => {
                ("InternalError".into(), http::status::StatusCode::INTERNAL_SERVER_ERROR)

            }
        };
        HttpResponseBuilder::new(code)
            .json(errstr)
    }
}

// The standard logger aborts ship when previous middleware returns an error.
// Rather than redo all my mw to return Ok(error response), I just made this.
// Also, jeez, so much typing for a simple reusable middleware ...
pub struct ErrorLoggerFactory;

impl <S, B>Transform<S, ServiceRequest> for ErrorLoggerFactory
    where
        S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
        S::Future: 'static,
        B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = ErrorLogger<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(ErrorLogger {
            service: Rc::new(service),

        }))
    }
}

pub struct ErrorLogger<S> {
    service: Rc<S>,
}

impl<S, B> Service<ServiceRequest> for ErrorLogger<S>
    where
        S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
        S::Future: 'static,
        B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = Rc::clone(&self.service);
        Box::pin(async move {
            let path = String::from(req.path());
            let res = service.call(req).await;
            match res {
                Ok(res) => Ok(res),
                Err(e) => {
                    log::error!("above error in middleware, when requesting {}", path);
                    Err(e)
                }
            }
        })
    }
}