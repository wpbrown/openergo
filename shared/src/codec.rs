use serde::{Serialize, de::DeserializeOwned};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

const MAX_MESSAGE_SIZE: usize = 64 * 1024;

pub struct PostcardCodec<T, U> {
    _phantom: std::marker::PhantomData<(T, U)>,
}

impl<T, U> Default for PostcardCodec<T, U> {
    fn default() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T, U> Clone for PostcardCodec<T, U> {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl<T, U> PostcardCodec<T, U> {
    fn check_message_size(len: usize) -> Result<(), io::Error> {
        if len > MAX_MESSAGE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("message size {len} exceeds maximum of {MAX_MESSAGE_SIZE}"),
            ));
        }
        Ok(())
    }
}

impl<T: DeserializeOwned, U> Decoder for PostcardCodec<T, U> {
    type Item = T;
    type Error = io::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }

        let len = u32::from_le_bytes([src[0], src[1], src[2], src[3]]) as usize;
        Self::check_message_size(len)?;
        if src.len() < 4 + len {
            src.reserve(4 + len - src.len());
            return Ok(None);
        }

        let _ = src.split_to(4);
        let data = src.split_to(len);

        postcard::from_bytes(&data)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

impl<T, U: Serialize> Encoder<U> for PostcardCodec<T, U> {
    type Error = io::Error;

    fn encode(&mut self, item: U, dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        use bytes::BufMut;

        // Reserve space for length prefix
        let len_offset = dst.len();
        dst.put_u32_le(0);

        // Serialize directly into buffer using writer
        let start = dst.len();
        let writer = dst.writer();
        postcard::to_io(&item, writer)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Write actual length back to reserved spot
        let len = dst.len() - start;
        Self::check_message_size(len)?;
        dst[len_offset..len_offset + 4].copy_from_slice(&(len as u32).to_le_bytes());

        Ok(())
    }
}
