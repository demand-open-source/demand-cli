use bitcoin::{
    consensus::{deserialize, serialize},
    hashes::Hash,
    Transaction, Txid,
};
use sqlx::Row;
use tracing::{error, info, warn};

/// Reload stored prioritized transactions on startup.
pub async fn restore() {
    let Some(pool) = crate::db::pool() else {
        return;
    };

    let rows = match sqlx::query("SELECT txid, tx FROM prioritized_transactions")
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            error!(%error, "could not read the prioritised transactions back");
            return;
        }
    };

    let mut restored = 0;
    for row in rows {
        let raw: Vec<u8> = row.get("tx");
        match deserialize::<Transaction>(&raw) {
            Ok(transaction) => {
                crate::prioritized_transactions::record(transaction);
                restored += 1;
            }
            Err(error) => {
                warn!(%error, "a stored prioritised transaction would not decode");
            }
        }
    }

    if restored > 0 {
        info!(restored, "restored the prioritised transactions");
    }
}

/// Store one, so it survives a restart.
pub async fn insert(txid: Txid, transaction: &Transaction) {
    let Some(pool) = crate::db::pool() else {
        return;
    };
    if let Err(error) = sqlx::query(
        "INSERT OR REPLACE INTO prioritized_transactions (txid, tx, created_at) VALUES (?, ?, ?)",
    )
    .bind(txid.to_byte_array().to_vec())
    .bind(serialize(transaction))
    .bind(crate::block_templates::unix_now() as i64)
    .execute(pool)
    .await
    {
        error!(%error, %txid, "could not store the prioritised transaction");
    }
}

/// Forget one, because bitcoind no longer has it in the mempool.
pub async fn delete(txid: Txid) {
    let Some(pool) = crate::db::pool() else {
        return;
    };
    if let Err(error) = sqlx::query("DELETE FROM prioritized_transactions WHERE txid = ?")
        .bind(txid.to_byte_array().to_vec())
        .execute(pool)
        .await
    {
        error!(%error, %txid, "could not drop the prioritised transaction");
    }
}
