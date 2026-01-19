use ekv::{Database, flash::Flash};
use embassy_sync_06::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex},
    mutex::Mutex,
};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_storage::FlashStorageError;
use static_cell::StaticCell;

use crate::sensors;

pub static DB: StaticCell<Mutex<CriticalSectionRawMutex, DbProxy>> = StaticCell::new();

const FLASH_BASE: usize = 0x600000;

pub struct EspFlash<T: NorFlash + ReadNorFlash> {
    storage: T,
}

impl<T: NorFlash + ReadNorFlash> Flash for EspFlash<T> {
    type Error = T::Error;

    fn page_count(&self) -> usize {
        ekv::config::MAX_PAGE_COUNT
    }

    async fn erase(&mut self, page_id: ekv::flash::PageID) -> Result<(), Self::Error> {
        let addr = page_addr(page_id);

        self.storage
            .erase(addr as u32, (addr + ekv::config::PAGE_SIZE) as u32)
    }

    async fn read(
        &mut self,
        page_id: ekv::flash::PageID,
        offset: usize,
        data: &mut [u8],
    ) -> Result<(), Self::Error> {
        let addr = page_addr(page_id) + offset;
        self.storage.read(addr as u32, data)
    }

    async fn write(
        &mut self,
        page_id: ekv::flash::PageID,
        offset: usize,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        let addr = page_addr(page_id) + offset;
        self.storage.write(addr as u32, data)
    }
}

fn page_addr(page_id: ekv::flash::PageID) -> usize {
    FLASH_BASE + page_id.index() * ekv::config::PAGE_SIZE
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
    ReadError(ekv::ReadError<FlashStorageError>),
    WriteError(ekv::WriteError<FlashStorageError>),
    CommitError(ekv::CommitError<FlashStorageError>),
    SerializationError(postcard::Error),
    FormatError(ekv::FormatError<FlashStorageError>),
}

impl From<ekv::ReadError<FlashStorageError>> for DbError {
    fn from(err: ekv::ReadError<FlashStorageError>) -> Self {
        DbError::ReadError(err)
    }
}

impl From<ekv::WriteError<FlashStorageError>> for DbError {
    fn from(err: ekv::WriteError<FlashStorageError>) -> Self {
        DbError::WriteError(err)
    }
}

impl From<ekv::CommitError<FlashStorageError>> for DbError {
    fn from(err: ekv::CommitError<FlashStorageError>) -> Self {
        DbError::CommitError(err)
    }
}

impl From<postcard::Error> for DbError {
    fn from(err: postcard::Error) -> Self {
        DbError::SerializationError(err)
    }
}

impl From<ekv::FormatError<FlashStorageError>> for DbError {
    fn from(err: ekv::FormatError<FlashStorageError>) -> Self {
        DbError::FormatError(err)
    }
}

pub struct DbProxy {
    db: Database<EspFlash<esp_storage::FlashStorage<'static>>, NoopRawMutex>,
}

impl DbProxy {
    pub fn new(flash: esp_hal::peripherals::FLASH<'static>) -> Self {
        let flash = EspFlash {
            storage: esp_storage::FlashStorage::new(flash),
        };

        Self {
            db: Database::new(flash, ekv::Config::default()),
        }
    }

    pub async fn store(&mut self, value: Value) -> Result<Key, DbError> {
        let key = self.next_key().await?;

        let mut buf = [0u8; ekv::config::MAX_VALUE_SIZE];
        let data = postcard::to_slice(&value, &mut buf)?;

        let mut tx = self.db.write_transaction().await;
        tx.write(&key.as_bytes(), data).await?;
        tx.commit().await?;

        Ok(key)
    }

    pub async fn get(&self, key: Key) -> Result<Option<Value>, DbError> {
        let tx = self.db.read_transaction().await;

        let mut buf = [0u8; ekv::config::MAX_VALUE_SIZE];
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

pub async fn init(flash: esp_hal::peripherals::FLASH<'static>) -> Result<(), DbError> {
    let db = DbProxy::new(flash);
    let db = DB.init(Mutex::new(db));

    let db = db.get_mut();
    if db.db.mount().await.is_err() {
        db.db.format().await?;
    }

    Ok(())
}
