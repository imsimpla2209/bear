
use std::future::Future;
use std::sync::{Mutex};

use actix_web::dev::ServerHandle;

use crate::db::DbMain;
use crate::txnmw::Switcharoo;

static SERVER_HANDLE: Mutex<Option<ServerHandle>> = Mutex::new(None);

pub fn err_expected<T: ToString>(act: Option<T>, exp: &str) -> bool {
    let s = match act {
        None => {
            log::error!("Expected err '{}' but got success.", exp);
            return false
        },
        Some(e) => e.to_string()
    };
    let has = s.contains(exp);
    if !has {
        log::error!("Expected '{}' but received different error message: '{}'", exp, s);
    }
    has
}

pub fn put_server_handle(h: ServerHandle) {
    let mut handle = SERVER_HANDLE.lock().unwrap();
    if handle.is_some() { panic!("leftover") }
    *handle = Some(h);
}

// kill any old server
pub async fn kill_server() {
    let mut oldsvr = SERVER_HANDLE.lock().unwrap();
    if let Some(ref svr_it) = *oldsvr {
        svr_it.stop(true).await;
        //sleep(Duration::from_secs(8)).await;
        *oldsvr = None
    }
}

// not returning Result (just panic) for easier handling and checking of errors of the inner block
pub async fn handler_with_tx<'a, F: Fn(Switcharoo<'a>) -> K, K: Future<Output=R>, R>(db: &DbMain, block: F) -> R {
    let (txn, txco) = Switcharoo::from_tx(db.newtx_read().await).unwrap().into_tuple();
    let res = block(txn).await;
    txco.take().unwrap().commit().await.unwrap();
    res
}
/* lifetimes too complex
pub async fn with_tx<'a, 'b, F, K>(db: &DbMain, block: F) -> anyhow::Result<R>
    where
        'a: 'b,
        F: Fn(&mut DbWriteTxn<'_>) -> K,
        K: Future<Output=()>
{
    let mut txn = db.newtx_write().await.unwrap();
    let res = block(&mut txn).await;
    let _ = txn.commit().await.unwrap();
    Ok(res)
}
 */
