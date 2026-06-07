use bachelor::error::Closed;
use bitflags::Flags;
use futures::FutureExt;
use futures::future::Either;
use std::future::{Future, Pending};

pub(crate) trait FiniteChanges {
    fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_;
}

pub(crate) trait ChangeSet {
    type Key: Flags + Copy;

    fn changed(
        &mut self,
        open: Self::Key,
    ) -> impl Future<Output = Result<Self::Key, Self::Key>> + '_;
}

pub(crate) struct WatchMux<T: ChangeSet> {
    inputs: T,
    open: T::Key,
}

impl<T: ChangeSet> WatchMux<T> {
    pub(crate) fn new(inputs: T) -> Self {
        Self {
            inputs,
            open: T::Key::all(),
        }
    }

    pub(crate) fn get(&self) -> &T {
        &self.inputs
    }

    pub(crate) async fn changed(&mut self) -> Result<T::Key, Closed> {
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

    pub(crate) async fn closed(&mut self) {
        while self.changed().await.is_ok() {}
    }
}

pub(crate) fn changed_if_open<C, K>(
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

macro_rules! define_watch_mux_4 {
    (
        $vis:vis struct $inputs:ident;
        $flags_vis:vis flags $flags:ident;
        $field1:ident : $ty1:ty => $flag1:ident,
        $field2:ident : $ty2:ty => $flag2:ident,
        $field3:ident : $ty3:ty => $flag3:ident,
        $field4:ident : $ty4:ty => $flag4:ident $(,)?
    ) => {
        $vis struct $inputs {
            pub $field1: $ty1,
            pub $field2: $ty2,
            pub $field3: $ty3,
            pub $field4: $ty4,
        }

        ::bitflags::bitflags! {
            #[derive(Clone, Copy)]
            $flags_vis struct $flags: u8 {
                const $flag1 = 0b0001;
                const $flag2 = 0b0010;
                const $flag3 = 0b0100;
                const $flag4 = 0b1000;
            }
        }

        impl $crate::watch_mux::ChangeSet for $inputs {
            type Key = $flags;

            fn changed(
                &mut self,
                open: Self::Key,
            ) -> impl ::std::future::Future<Output = Result<Self::Key, Self::Key>> + '_ {
                async move {
                    use ::futures::future::{select, Either};

                    let $field1 = $crate::watch_mux::changed_if_open(
                        open,
                        $flags::$flag1,
                        &mut self.$field1,
                    );
                    let $field2 = $crate::watch_mux::changed_if_open(
                        open,
                        $flags::$flag2,
                        &mut self.$field2,
                    );
                    let $field3 = $crate::watch_mux::changed_if_open(
                        open,
                        $flags::$flag3,
                        &mut self.$field3,
                    );
                    let $field4 = $crate::watch_mux::changed_if_open(
                        open,
                        $flags::$flag4,
                        &mut self.$field4,
                    );
                    let any_change = select(
                        select($field1, $field2),
                        select($field3, $field4),
                    );
                    match any_change.await {
                        Either::Left((Either::Left((res, _)), _))
                        | Either::Left((Either::Right((res, _)), _))
                        | Either::Right((Either::Left((res, _)), _))
                        | Either::Right((Either::Right((res, _)), _)) => res,
                    }
                }
            }
        }
    };
}

pub(crate) use define_watch_mux_4;
