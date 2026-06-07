use super::analog::{
    AnalogIn, AnalogInProducer, AnalogInSource, AnalogOut, AnalogOutProducer, EndpointIo,
    analog_in_pair, analog_out_pair,
};
use super::endpoints::{Direction, EndpointCatalog, EndpointConfig, EndpointLabel};
use std::collections::HashMap;
use std::collections::hash_map::Entry;

/// A single endpoint bound by [`Binder::complete`]. Pairs the label
/// handle with the catalog entry (typed by the application's payload
/// `T`) and the producer/consumer halves the transport needs.
pub struct EndpointBinding<T> {
    pub label: EndpointLabel,
    pub config: T,
    pub io: EndpointIo,
}

#[derive(Debug)]
pub enum BindError {
    Unknown(String),
    DirectionMismatch {
        label: String,
        wanted: &'static str,
        actual: Direction,
    },
    /// Returned by [`Binder::analog_out`] when the same label has
    /// already been bound as an output. Outputs are exclusive (multiple
    /// writers would race meaninglessly); inputs fan out and never
    /// return this error.
    AlreadyBound {
        label: String,
    },
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindError::Unknown(label) => write!(f, "unknown control label '{label}'"),
            BindError::DirectionMismatch {
                label,
                wanted,
                actual,
            } => write!(
                f,
                "control '{label}' has direction {actual:?}; cannot bind as {wanted}"
            ),
            BindError::AlreadyBound { label } => write!(
                f,
                "control '{label}' is already bound as AnalogOut; only one writer is allowed per output"
            ),
        }
    }
}

impl std::error::Error for BindError {}

/// Internal scratch storage for a label that's been bound as an input.
/// The shared [`AnalogInSource`] lets [`Binder::analog_in`] fan out
/// additional consumers without re-allocating the watch; the source is
/// dropped at [`Binder::complete`] time and only the producer flows on
/// to the transport.
struct InBinding {
    producer: AnalogInProducer,
    source: AnalogInSource,
}

pub struct Binder<T> {
    catalog: EndpointCatalog<T>,
    ins: HashMap<EndpointLabel, InBinding>,
    outs: HashMap<EndpointLabel, AnalogOut>,
}

impl<T: EndpointConfig> Binder<T> {
    pub fn new(catalog: EndpointCatalog<T>) -> Self {
        Self {
            catalog,
            ins: HashMap::new(),
            outs: HashMap::new(),
        }
    }

    /// Borrow the leaked label store. Convenient when callers need to
    /// resolve labels after [`Self::complete`] has consumed the binder
    /// (and with it the catalog).
    pub fn labels(&self) -> &'static super::endpoints::EndpointLabelStore {
        self.catalog.labels()
    }

    /// Allocate (or fan out from) a watch for `label` (which must have
    /// an `In` or `InOut` direction in the catalog) and return a freshly
    /// subscribed domain-side consumer. Multiple callers binding the
    /// same label all read the same transport-written value.
    pub fn analog_in(&mut self, label: &str) -> Result<AnalogIn, BindError> {
        let label_handle = self.lookup(label, Direction::allows_in, "AnalogIn")?;
        let analog_in = match self.ins.entry(label_handle) {
            Entry::Occupied(slot) => slot.get().source.subscribe(),
            Entry::Vacant(slot) => {
                let (producer, source) = analog_in_pair();
                let analog_in = source.subscribe();
                slot.insert(InBinding { producer, source });
                analog_in
            }
        };
        Ok(analog_in)
    }

    /// Allocate a watch for `label` (which must have an `Out` or `InOut`
    /// direction in the catalog) seeded with `initial`, and return the
    /// domain-side producer. The seed is what the transport reads on
    /// its first publish, before the domain has had a chance to update
    /// the watch.
    ///
    /// Outputs are exclusive: a second call for the same label returns
    /// [`BindError::AlreadyBound`].
    pub fn analog_out(
        &mut self,
        label: &str,
        initial: f64,
    ) -> Result<AnalogOutProducer, BindError> {
        let label_handle = self.lookup(label, Direction::allows_out, "AnalogOut")?;
        match self.outs.entry(label_handle) {
            Entry::Occupied(_) => Err(BindError::AlreadyBound {
                label: label.to_string(),
            }),
            Entry::Vacant(slot) => {
                let (producer, analog_out) = analog_out_pair(initial);
                slot.insert(analog_out);
                Ok(producer)
            }
        }
    }

    /// Consume the binder, returning every bound endpoint paired with
    /// its catalog configuration. In + Out halves of the same label are
    /// collapsed into a single [`EndpointIo::InOut`]. The owned `T` is
    /// moved out of the catalog; unbound entries are dropped along
    /// with the catalog when this method returns.
    pub fn complete(self) -> Vec<EndpointBinding<T>> {
        let Self {
            mut catalog,
            mut ins,
            outs,
        } = self;
        let mut result: Vec<EndpointBinding<T>> = Vec::with_capacity(ins.len() + outs.len());
        for (label, output) in outs {
            let config = catalog
                .take(label)
                .expect("label was bound, so it must be in the catalog");
            let io = match ins.remove(&label) {
                Some(InBinding { producer, .. }) => EndpointIo::InOut {
                    input: producer,
                    output,
                },
                None => EndpointIo::Out(output),
            };
            result.push(EndpointBinding { label, config, io });
        }
        for (label, InBinding { producer, .. }) in ins {
            let config = catalog
                .take(label)
                .expect("label was bound, so it must be in the catalog");
            result.push(EndpointBinding {
                label,
                config,
                io: EndpointIo::In(producer),
            });
        }
        result
    }

    fn lookup(
        &self,
        label: &str,
        allows: fn(Direction) -> bool,
        wanted: &'static str,
    ) -> Result<EndpointLabel, BindError> {
        let handle = self
            .catalog
            .labels()
            .get(label)
            .ok_or_else(|| BindError::Unknown(label.to_string()))?;
        let entry = self
            .catalog
            .lookup(handle)
            .ok_or_else(|| BindError::Unknown(label.to_string()))?;
        if !allows(entry.direction()) {
            return Err(BindError::DirectionMismatch {
                label: label.to_string(),
                wanted,
                actual: entry.direction(),
            });
        }
        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration::endpoints::EndpointLabelStore;
    use litemap::LiteMap;

    /// Minimal `EndpointConfig` impl for tests; carries only the
    /// direction the binder needs.
    struct TestEntry {
        direction: Direction,
    }

    impl EndpointConfig for TestEntry {
        fn direction(&self) -> Direction {
            self.direction
        }
    }

    fn build_catalog() -> EndpointCatalog<TestEntry> {
        let mut labels = EndpointLabelStore::new();
        let in_label = labels.get_or_intern("left_index");
        let out_label = labels.get_or_intern("led_rest");
        let labels: &'static EndpointLabelStore = Box::leak(Box::new(labels));

        let mut by_label = LiteMap::new();
        by_label.insert(
            in_label,
            TestEntry {
                direction: Direction::In,
            },
        );
        by_label.insert(
            out_label,
            TestEntry {
                direction: Direction::Out,
            },
        );

        EndpointCatalog::new(labels, by_label)
    }

    #[test]
    fn binder_returns_bound_endpoints_with_catalog_entries() {
        let catalog = build_catalog();
        let in_label = catalog
            .labels()
            .get("left_index")
            .expect("left_index is in the catalog");
        let out_label = catalog
            .labels()
            .get("led_rest")
            .expect("led_rest is in the catalog");
        let mut binder = Binder::new(catalog);

        let mut analog_in = binder
            .analog_in("left_index")
            .expect("left_index is bindable as AnalogIn");
        let _out_producer = binder
            .analog_out("led_rest", 0.42)
            .expect("led_rest is bindable as AnalogOut");

        let bound = binder.complete();
        assert_eq!(bound.len(), 2);
        let mut by_label: HashMap<EndpointLabel, EndpointBinding<TestEntry>> =
            bound.into_iter().map(|b| (b.label, b)).collect();

        let EndpointBinding {
            config: in_entry,
            io: in_io,
            ..
        } = by_label.remove(&in_label).expect("left_index bound");
        assert_eq!(in_entry.direction, Direction::In);
        match in_io {
            EndpointIo::In(producer) => {
                producer.set(0.7).expect("AnalogIn watch is open");
                assert_eq!(analog_in.get(), 0.7);
                let _ = futures::FutureExt::now_or_never(analog_in.changed());
            }
            _ => panic!("expected EndpointIo::In"),
        }

        let EndpointBinding {
            config: out_entry,
            io: out_io,
            ..
        } = by_label.remove(&out_label).expect("led_rest bound");
        assert_eq!(out_entry.direction, Direction::Out);
        match out_io {
            EndpointIo::Out(consumer) => {
                // Transport side starts with the seed for AnalogOut.
                assert_eq!(consumer.get(), 0.42);
            }
            _ => panic!("expected EndpointIo::Out"),
        }
    }

    #[test]
    fn binder_rejects_unknown_label() {
        let catalog = build_catalog();
        let mut binder = Binder::new(catalog);
        assert!(matches!(
            binder.analog_in("nope"),
            Err(BindError::Unknown(ref s)) if s == "nope"
        ));
        assert!(matches!(
            binder.analog_out("also_nope", 0.0),
            Err(BindError::Unknown(ref s)) if s == "also_nope"
        ));
    }

    #[test]
    fn binder_rejects_direction_mismatch() {
        let catalog = build_catalog();
        let mut binder = Binder::new(catalog);
        // led_rest is Out → trying to bind as AnalogIn must fail.
        assert!(matches!(
            binder.analog_in("led_rest"),
            Err(BindError::DirectionMismatch { .. })
        ));
        // left_index is In → trying to bind as AnalogOut must fail.
        assert!(matches!(
            binder.analog_out("left_index", 0.0),
            Err(BindError::DirectionMismatch { .. })
        ));
    }

    #[test]
    fn binder_fans_out_analog_in_to_multiple_consumers() {
        let catalog = build_catalog();
        let in_label = catalog
            .labels()
            .get("left_index")
            .expect("left_index is in the catalog");
        let mut binder = Binder::new(catalog);

        let mut a = binder.analog_in("left_index").expect("first bind");
        let mut b = binder
            .analog_in("left_index")
            .expect("second bind fans out");

        let mut bound = binder.complete();
        assert_eq!(bound.len(), 1);
        let EndpointBinding { label, io, .. } = bound.pop().expect("bound");
        assert_eq!(label, in_label);
        let EndpointIo::In(producer) = io else {
            panic!("expected EndpointIo::In");
        };

        producer.set(0.5).expect("watch open");
        assert_eq!(a.get(), 0.5);
        assert_eq!(b.get(), 0.5);
        let _ = futures::FutureExt::now_or_never(a.changed());
        let _ = futures::FutureExt::now_or_never(b.changed());
    }

    #[test]
    fn binder_rejects_double_analog_out() {
        let catalog = build_catalog();
        let mut binder = Binder::new(catalog);
        let _first = binder
            .analog_out("led_rest", 0.0)
            .expect("first analog_out succeeds");
        assert!(matches!(
            binder.analog_out("led_rest", 0.0),
            Err(BindError::AlreadyBound { ref label }) if label == "led_rest"
        ));
    }
}
