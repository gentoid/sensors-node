use ekv::{Database, FormatError, flash::Flash};
use embassy_sync_06::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex},
    mutex::Mutex,
};
use static_cell::StaticCell;

use crate::sensors;

pub static DB: StaticCell<Mutex<CriticalSectionRawMutex, DbProxy>> = StaticCell::new();

const SAMPLE_SIZE: usize = 256;

pub struct EspFlash;

#[derive(Debug, defmt::Format)]
pub struct FlashError;

impl Flash for EspFlash {
    type Error = FlashError;

    fn page_count(&self) -> usize {
        todo!()
    }

    async fn erase(&mut self, page_id: ekv::flash::PageID) -> Result<(), Self::Error> {
        todo!()
    }

    async fn read(
        &mut self,
        page_id: ekv::flash::PageID,
        offset: usize,
        data: &mut [u8],
    ) -> Result<(), Self::Error> {
        todo!()
    }

    async fn write(
        &mut self,
        page_id: ekv::flash::PageID,
        offset: usize,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        todo!()
    }
}

pub struct Key(u32);

impl From<[u8; 4]> for Key {
    fn from(value: [u8; 4]) -> Self {
        Self(u32::from_le_bytes(value))
    }
}

impl Into<[u8; 4]> for Key {
    fn into(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }
}

impl Key {
    fn next(&self) -> Self {
        Self(self.0 + 1)
    }

    fn as_bytes(&self) -> [u8; 4] {
        self.0.to_le_bytes()
    }
}

type Value = sensors::Sample;

#[derive(Debug, defmt::Format)]
pub enum DbError {
    ReadError(ekv::ReadError<FlashError>),
    WriteError(ekv::WriteError<FlashError>),
    CommitError(ekv::CommitError<FlashError>),
    SerializationError(postcard::Error),
}

impl From<ekv::ReadError<FlashError>> for DbError {
    fn from(err: ekv::ReadError<FlashError>) -> Self {
        DbError::ReadError(err)
    }
}

impl From<ekv::WriteError<FlashError>> for DbError {
    fn from(err: ekv::WriteError<FlashError>) -> Self {
        DbError::WriteError(err)
    }
}

impl From<ekv::CommitError<FlashError>> for DbError {
    fn from(err: ekv::CommitError<FlashError>) -> Self {
        DbError::CommitError(err)
    }
}

impl From<postcard::Error> for DbError {
    fn from(err: postcard::Error) -> Self {
        DbError::SerializationError(err)
    }
}

pub struct DbProxy {
    db: Database<EspFlash, NoopRawMutex>,
}

impl DbProxy {
    pub fn new() -> Self {
        Self {
            db: Database::new(EspFlash, ekv::Config::default()),
        }
    }

    pub async fn store(&mut self, value: Value) -> Result<Key, DbError> {
        let key = self.next_key().await?;

        let mut buf = [0u8; SAMPLE_SIZE];
        let data = postcard::to_slice(&value, &mut buf)?;

        let mut tx = self.db.write_transaction().await;
        tx.write(&key.as_bytes(), data).await?;
        tx.commit().await?;

        Ok(key)
    }

    pub async fn get(&self, key: Key) -> Result<Option<Value>, DbError> {
        let tx = self.db.read_transaction().await;

        let mut buf = [0u8; SAMPLE_SIZE];
        tx.read(&key.as_bytes(), &mut buf).await?;

        Ok(postcard::from_bytes(&buf)?)
    }

    pub async fn drop(&mut self, key: Key) -> Result<(), DbError> {
        let mut tx = self.db.write_transaction().await;
        tx.delete(&key.as_bytes()).await?;
        tx.commit().await?;

        Ok(())
    }

    async fn next_key(&mut self) -> Result<Key, DbError> {
        const KEY_ID: &'static [u8; 9] = b"_next_id_";

        let tx = self.db.read_transaction().await;
        let mut buf = [0u8; 4];

        tx.read(KEY_ID, &mut buf).await?;
        let id: Key = buf.into();

        let mut tx = self.db.write_transaction().await;
        tx.write(KEY_ID, &id.next().as_bytes()).await?;
        tx.commit().await?;

        Ok(id)
    }
}

pub async fn init() -> Result<(), FormatError<FlashError>> {
    let db = DbProxy::new();
    let db = DB.init(Mutex::new(db));

    let db = db.get_mut();
    if db.db.mount().await.is_err() {
        db.db.format().await?;
    }

    Ok(())
}
