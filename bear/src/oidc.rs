use std::fmt::Display;
use actix_web::{cookie, HttpRequest, HttpResponseBuilder, Responder, web};

use anyhow::anyhow;
use hex::ToHex;
use http::StatusCode;
use inth_oauth2::Token;
use oidc::token::Jws;
use serde::Deserialize;
use crate::cfg::ServerSettings;
use crate::errors::{AnyHandlerError, ApiError};
use crate::interface::{AppContainer, CommonSecretKind};
use crate::txnmw::WriteTxn;
use crate::utils::{gentoken, std_cookie};
use crate::interface::Session;
use crate::cfg::Cfg;

pub const SESSION_COOKIE_NAME: &str = "session";
pub const OIDC_NONCE_COOKIE_NAME: &str = "oidc_nonce";

// actually needs to match with SessionKind in earn project
pub const SESSION_KIND_OIDC: &str = "Oidc";

fn make_client(cfg: &ServerSettings, secret: String) -> anyhow::Result<oidc::Client> {
    let id = cfg.oidc_client_id.clone();
    let redirect = url::Url::parse(&format!("{}/api/oidc/callback", cfg.public_url)).map_err(anyhow::Error::from)?;
    let issuer = oidc::issuer::google();
    let client = oidc::Client::discover(id, secret, redirect, issuer).map_err(anyerr)?;
    Ok(client)
}

pub async fn oidc_start<AC: AppContainer + 'static>(objs: web::Data<AC>) -> Result<impl Responder, AnyHandlerError> {
    let nonce_preimage: String = gentoken();
    let nonce_preimage_cl = nonce_preimage.clone();

    let task = tokio::task::spawn_blocking(move || {
        let nonce = ring::digest::digest(&ring::digest::SHA256, bs58::decode(nonce_preimage_cl).into_vec().unwrap().as_slice());
        let client = make_client(&objs.cfg().server(), objs.secret(CommonSecretKind::OidcSecret).into())?;
        let mut opts = oidc::Options::default();
        opts.scope = Some("openid email profile".to_string());
        opts.state = Some(gentoken());
        opts.nonce = Some(nonce.encode_hex());
        Ok::<url::Url, anyhow::Error>(client.auth_url(&opts))
    });
    let auth_url = task.await.map_err(|e| anyhow!("join error{e}"))??;

    Ok(HttpResponseBuilder::new(StatusCode::FOUND)
        .append_header(("Location", auth_url.to_string()))
        .cookie(cookie::Cookie::build(OIDC_NONCE_COOKIE_NAME, nonce_preimage)
            .path("/")
            .http_only(true)
            .finish())
        .finish())
}

#[derive(Debug, Deserialize)]
pub struct OidcCallbackQuery {
    code: String,
}

fn anyerr<T: Display>(e: T) -> anyhow::Error {
    anyhow::anyhow!("{:}", e)
}

// BTW getting user info manually because the lib has an old version of reqwest in the interface so can't use it
pub async fn oidc_callback<AC>(
    objs: web::Data<AC>,
    mut txn: WriteTxn<'_>,
    req: HttpRequest,
    query: web::Query<OidcCallbackQuery>
) -> Result<impl Responder, AnyHandlerError>
    where AC: AppContainer + 'static,
            AC::S: Session + 'static,
            AC::Cfg: Cfg + 'static
{
    let nonce_preimage = req.cookie(OIDC_NONCE_COOKIE_NAME)
            .ok_or(ApiError::AuthError)?;

    let exp_nonce = ring::digest::digest(
        &ring::digest::SHA256,
        bs58::decode(nonce_preimage.value())
            .into_vec()
            .map_err(|_| ApiError::AuthError)?
            .as_slice()
    );

    let task  = tokio::task::spawn_blocking(move || {
        let client = make_client(&objs.cfg().server(), objs.secret(CommonSecretKind::OidcSecret).into())?;
        let http = reqwest::blocking::Client::new();
        let token = client.authenticate(&query.code, Some(&exp_nonce.encode_hex::<String>()), None)
            .map_err(anyerr)?;

        let disco_url = client.config().userinfo_endpoint.as_ref().unwrap();
        let userinfo: oidc::Userinfo = http.get(disco_url.as_str())
            .header("Authorization", format!("Bearer {}", token.access_token()))
            .send()
            .map_err(anyerr)?
            .json().map_err(anyerr)?;

        if let Jws::Decoded { header: _, payload: _pay } = token.id_token {
            if (userinfo.email_verified == false) || (userinfo.email.is_none()) { return Err(ApiError::AuthError.into()) }

            // no  accepting general users for now
            if !objs.cfg().server().oidc_admins.contains(userinfo.email.as_ref().unwrap()) { return Err(ApiError::AuthError.into()) }

            let now = objs.get_ref().utcnow();
            let sess = AC::S::new_oidc(now, userinfo.email.unwrap());
            // let sess = AC::Session::new_oidc(now + SessionKind::Oidc.lifetime(), userinfo.email.unwrap());


            //println!("query: {:?} token: {:?}", query, serde_json::to_string(&pay));
            //println!("userinfo: {:?}", userinfo);
            //sess.code
            Ok::<AC::S, anyhow::Error>(sess)


        } else {
            Err(ApiError::Any("oidc_callback.not_decoded".into()).into())
        }
    });

    let sess = task.await.map_err(|e| anyhow!("join error{e}"))??;
    log::info!("Storing session {sess:?}");
    AC::S::insert(txn.get(), &sess).await?;

    Ok(HttpResponseBuilder::new(StatusCode::FOUND)
        .append_header(("Location", "/en/admin"))
        .cookie(std_cookie(SESSION_COOKIE_NAME, sess.code()))
        .finish())
}
