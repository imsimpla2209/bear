use std::cell::{Cell};
use std::future::{Ready, ready};
use std::rc::Rc;
use actix_web::dev::{forward_ready, Payload, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::{Error, FromRequest, HttpMessage, HttpRequest, web};
use anyhow::anyhow;
use futures_util::future::LocalBoxFuture;
use crate::db::{DbMain, DbTxn, DbWriteTxn};
use crate::errors::AnyHandlerError;
use crate::errors::ApiError;

#[derive(Debug, PartialEq)]
pub enum TxnState {
    Nonexistent,
    Started,
    Committed,
    RolledBack
}

// .- For keeping track during testing .-
// #[cfg(test)]  // cant be conditional because we are in a library TODO fix this (and below)
pub static TXN_LOG: std::sync::Mutex<Vec<TxnState>> = std::sync::Mutex::new(Vec::new());

#[allow(unused)]
fn log_txn_state(state: TxnState) {
    // #[cfg(test)]
    TXN_LOG.lock().unwrap().push(state);
}

/// Transaction Middleware for actix_web and sqlx.
pub struct TxnMidFactory {
    pub pool: DbMain
}

impl TxnMidFactory {
    pub fn new(pool: DbMain) -> TxnMidFactory {
        TxnMidFactory {
            pool
        }
    }
}

impl <S, B>Transform<S, ServiceRequest> for TxnMidFactory
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = TxnMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(TxnMiddleware {
            service: Rc::new(service),
            pool: self.pool.clone()
        }))
    }
}

pub struct TxnMiddleware<S> {
    pub service: Rc<S>,
    pub pool: DbMain
}

impl<S, B> Service<ServiceRequest> for TxnMiddleware<S>
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
            // insert an empty tx container
            let cont: TxnRcContainer = Rc::new(Cell::new(None));
            req.extensions_mut().insert(Rc::clone(&cont));
            log_txn_state(TxnState::Nonexistent);
            //log::info!("Inserted empty transaction container, calling service");

            let res = service.call(req).await;
            if let Some(txn) = cont.take() {
                if let Ok(ref r) = res {
                    let failed = r.status().is_server_error() || r.status().is_client_error();
                    // log::debug!("Service result is Ok, status={status} failed={failed}");
                    if !failed {
                        log_txn_state(TxnState::Committed);
                        txn.commit().await

                    } else {
                        log_txn_state(TxnState::RolledBack);
                        txn.rollback().await
                    }
                } else {
                    //log::debug!("Service result is Error");
                    log_txn_state(TxnState::RolledBack);
                    txn.rollback().await
                }.map_err(|ref txerr| {
                    if let Err(ref e) = res {
                        log::error!("Transaction rollback failed. Also the original handler error was: {e:?}");
                    }
                    AnyHandlerError::from(anyhow!("SQL error on rollback/commit: {:?}", txerr))
                })?;

                //log::debug!("Transaction closed.");
            } else {
                // some handlers don't use transaction
                //log::warn!("No transaction in container");
            }

            Ok(res?)
        })
    }
}

/*
    The middleware needs to create a transaction, hand ownership of it temporarily
    to the handler and once that is done, take ownership back and commit or rollback.
    We employ a Cell and multiple pointers (Rc) to it. Additionally, the Switcharoo
    class keeps track of the local copy so that handler can conveniently call .get()
    multiple times without repeating any unnecessary work. The Option is used because
    Cell demands replacing its content with something else when transferring ownership.
    We just say either tx is there or not.
 */
pub type TxnRcContainer<'a> = Rc<Cell<Option<DbTxn<'a>>>>;

pub struct Switcharoo<'a> {
    shared: TxnRcContainer<'a>,
    local: Option<DbTxn<'a>>
}

pub struct TestTransactionHolder<'a> {
    pub for_handler: Switcharoo<'a>,
    pub for_context: TxnRcContainer<'a>,
}

impl<'a> Switcharoo<'a> {
    pub fn from_request(req: &HttpRequest) -> Result<(web::Data<DbMain>, Switcharoo<'static>), Error> {
        // get stuff
        let exts = req.extensions();
        let txcont: TxnRcContainer = exts.get::<TxnRcContainer>()
            .ok_or(Error::from(AnyHandlerError::from(ApiError::InvalidState(String::from("TxnContainer missing in req")))))?
            .clone();
        let db = req.app_data::<web::Data<DbMain>>()
            .ok_or(Error::from(AnyHandlerError::from(ApiError::InvalidState(String::from("Data<DbMain> missing in request")))))?
            .clone();

        Ok((db, Switcharoo {
            shared: txcont,
            local: None
        }))
    }

    // for tests
    #[allow(unused)]
    pub fn from_tx(txn: anyhow::Result<DbTxn<'a>>) -> anyhow::Result<TestTransactionHolder<'a>> {
        let tx_ok = txn?;
        let shared = Rc::new(Cell::new(Some(tx_ok)));
        let shared2 = Rc::clone(&shared);
        Ok(TestTransactionHolder {
            for_handler: Switcharoo {
                shared,
                local: None
            },
            for_context: shared2
        })
    }

    pub fn get<'x, 'y>(&'x mut self) -> &mut DbWriteTxn<'a> {
        if self.local.is_none() {
            let txn = self.shared.replace(None).unwrap();
            self.local = Some(txn);
            //log::info!("Moved from shared to local");
        }

        if let Some(ref mut txn) = self.local {
            txn
        } else {
            panic!("local is empty, unexpectedly");
        }
    }

    pub fn put(self, tx: anyhow::Result<DbTxn<'a>>) -> Result<Switcharoo, Error> {
        let txn = tx.map_err(|e| Error::from(AnyHandlerError::from(e)))?;
        log_txn_state(TxnState::Started);
        self.shared.set(Some(txn));
        //log::debug!("Started tx");

        Ok(self)
    }
}

impl Drop for Switcharoo<'_> {
    fn drop(&mut self) {
        if self.local.is_some() {
            self.shared.set(self.local.take());
            //log::info!("Drop: moved from local to shared");
        }
    }
}

impl<'a> TestTransactionHolder<'a> {

    pub fn into_tuple(self) -> (Switcharoo<'a>, TxnRcContainer<'a>) {
        (self.for_handler, self.for_context)
    }
}

pub struct WriteTxn<'a>(pub Switcharoo<'a>);
pub struct ReadTxn<'a>(pub Switcharoo<'a>);

impl FromRequest for WriteTxn<'static> {
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<WriteTxn<'static>, Error>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        match Switcharoo::from_request(req) {
            Err(e) => {
                return Box::pin(ready(Err(e)))
            },
            Ok((db, sw)) => {
                Box::pin(async move {
                    let txn = db.newtx_write().await;
                    Ok(WriteTxn(sw.put(txn)?))
                })
            }
        }
    }
}

impl<'a> WriteTxn<'a> {
    pub fn get<'x, 'y>(&'x mut self) -> &mut DbWriteTxn<'a> {
        self.0.get()
    }
}

impl<'a> ReadTxn<'a> {
    pub fn get<'x, 'y>(&'x mut self) -> &mut DbTxn<'a> {
        self.0.get()
    }
}

impl FromRequest for ReadTxn<'static> {
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<ReadTxn<'static>, Error>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        match Switcharoo::from_request(req) {
            Err(e) => {
                return Box::pin(ready(Err(e)))
            },
            Ok((db, sw)) => {
                Box::pin(async move {
                    let txn = db.newtx_read().await;
                    Ok(ReadTxn(sw.put(txn)?))
                })
            }
        }
    }
}

#[cfg(test)]
mod test {
}