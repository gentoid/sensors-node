#![no_std]
#![feature(impl_trait_in_assoc_type)]

use mqtt_client::packet::publish;

pub mod air_quality;
pub mod ble;
pub mod config;
pub mod dhcp;
#[cfg(feature = "display")]
pub mod display;
pub mod kv_storage;
pub mod led;
pub mod mqtt;
pub mod net_time;
pub mod sensors;
pub mod system;
pub mod web;
pub mod wifi;

#[derive(defmt::Format)]
enum Error {
    CannotConvertPayload,
}

#[derive(defmt::Format)]
pub(crate) enum Command {
    RebootToReconfigure,
}

impl<'a> TryFrom<publish::Publish<'a>> for Command {
    type Error = Error;

    fn try_from(msg: publish::Publish<'a>) -> Result<Self, Self::Error> {
        if msg.payload.len() != 1 {
            return Err(Error::CannotConvertPayload);
        }

        let value = msg.payload.as_bytes()[0];

        match value {
            48 => Ok(Self::RebootToReconfigure), // ASCII zero
            _ => Err(Error::CannotConvertPayload),
        }
    }
}
