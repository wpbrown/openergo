use bachelor::error::Closed;
use bitflags::Flags;
use futures::FutureExt;
use futures::future::Either;
use std::future::{Future, Pending};

pub trait FiniteChanges {
    fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_;
}

impl<T: FiniteChanges> FiniteChanges for Option<T> {
    fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_ {
        match self {
            Some(inner) => Either::Left(inner.changed()),
            None => Either::Right(std::future::ready(Err(Closed))),
        }
    }
}

pub trait ChangeSet {
    type Key: Flags + Copy;

    fn changed(
        &mut self,
        open: Self::Key,
    ) -> impl Future<Output = Result<Self::Key, Self::Key>> + '_;
}

pub struct WatchMux<T: ChangeSet> {
    inputs: T,
    open: T::Key,
}

impl<T: ChangeSet> WatchMux<T> {
    pub fn new(inputs: T) -> Self {
        Self {
            inputs,
            open: T::Key::all(),
        }
    }

    pub fn get(&self) -> &T {
        &self.inputs
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inputs
    }

    pub async fn changed(&mut self) -> Result<T::Key, Closed> {
        loop {
            match self.inputs.changed(self.open).await {
                Ok(changed) => return Ok(changed),
                Err(closed) => {
                    self.open.remove(closed);
                    if self.open.is_empty() {
                        return Err(Closed);
                    }
                }
            }
        }
    }

    pub async fn closed(&mut self) {
        while self.changed().await.is_ok() {}
    }
}

pub fn changed_if_open<C, K>(
    open: K,
    key: K,
    consumer: &mut C,
) -> Either<impl Future<Output = Result<K, K>> + '_, Pending<Result<K, K>>>
where
    C: FiniteChanges,
    K: Flags + Copy,
{
    if open.contains(key) {
        Either::Left(
            consumer
                .changed()
                .map(move |res| res.map(|()| key).map_err(|Closed| key)),
        )
    } else {
        Either::Right(std::future::pending())
    }
}

macro_rules! define_watch_mux {
    (
        $vis:vis struct $inputs:ident;
        $flags_vis:vis flags $flags:ident;
        $(
            $field:ident : $ty:ty => $flag:ident
        ),+ $(,)?
    ) => {
        define_watch_mux! {
            @collect
            [$vis, $inputs, $flags_vis, $flags]
            []
            1u8;
            $($field : $ty => $flag),+
        }
    };

    (
        @collect
        [$vis:vis, $inputs:ident, $flags_vis:vis, $flags:ident]
        [$($collected:tt)*]
        $bit:expr;
        $field:ident : $ty:ty => $flag:ident,
        $($rest:tt)+
    ) => {
        define_watch_mux! {
            @collect
            [$vis, $inputs, $flags_vis, $flags]
            [$($collected)* ($field, $ty, $flag, $bit)]
            ($bit << 1);
            $($rest)+
        }
    };

    (
        @collect
        [$vis:vis, $inputs:ident, $flags_vis:vis, $flags:ident]
        [$($collected:tt)*]
        $bit:expr;
        $field:ident : $ty:ty => $flag:ident
    ) => {
        define_watch_mux! {
            @emit
            [$vis, $inputs, $flags_vis, $flags]
            [$($collected)* ($field, $ty, $flag, $bit)]
        }
    };

    (
        @emit
        [$vis:vis, $inputs:ident, $flags_vis:vis, $flags:ident]
        [$(($field:ident, $ty:ty, $flag:ident, $bit:expr))*]
    ) => {
        $vis struct $inputs {
            $(
                pub $field: $ty,
            )+
        }

        ::bitflags::bitflags! {
            #[derive(Clone, Copy, PartialEq, Eq)]
            $flags_vis struct $flags: u8 {
                $(
                    const $flag = $bit;
                )+
            }
        }

        impl $crate::watch_mux::ChangeSet for $inputs {
            type Key = $flags;

            fn changed(
                &mut self,
                open: Self::Key,
            ) -> impl ::std::future::Future<Output = Result<Self::Key, Self::Key>> + '_ {
                async move {
                    $(
                        let mut $field = $crate::watch_mux::changed_if_open(
                            open,
                            $flags::$flag,
                            &mut self.$field,
                        );
                    )+

                    ::std::future::poll_fn(|cx| {
                        $(
                            if let ::std::task::Poll::Ready(res) =
                                ::std::pin::Pin::new(&mut $field).poll(cx)
                            {
                                return ::std::task::Poll::Ready(res);
                            }
                        )+
                        ::std::task::Poll::Pending
                    })
                    .await
                }
            }
        }
    };
}

pub(crate) use define_watch_mux;
