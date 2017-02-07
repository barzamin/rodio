//! Queue that plays sounds one after the other.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::time::Duration;

use source::Empty;
use source::Source;
use source::Zero;

use Sample;

/// Builds a new queue. It consists of an input and an output.
///
/// The input can be used to add sounds to the end of the queue, while the output implements
/// `Source` and plays the sounds.
///
/// The parameter indicates how the queue should behave if the queue becomes empty:
///
/// - If you pass `true`, then the queue is infinite and will play a silence instead until you add
///   a new sound.
/// - If you pass `false`, then the queue will report that it has finished playing.
///
pub fn queue<S>(keep_alive_if_empty: bool)
                -> (Arc<SourcesQueueInput<S>>, SourcesQueueOutput<S>)
    where S: Sample + Send + 'static
{
    let input = Arc::new(SourcesQueueInput {
        next_sounds: Mutex::new(Vec::new()),
    });

    let output = SourcesQueueOutput {
        current: Box::new(Empty::<S>::new()) as Box<_>,
        signal_after_end: None,
        input: input.clone(),
        keep_alive_if_empty: keep_alive_if_empty,
    };

    (input, output)
}

/// The input of the queue.
pub struct SourcesQueueInput<S> {
    next_sounds: Mutex<Vec<(Box<Source<Item = S> + Send>, Option<Sender<()>>)>>,
}

impl<S> SourcesQueueInput<S> where S: Sample + Send + 'static {
    /// Adds a new source to the end of the queue.
    #[inline]
    pub fn append<T>(&self, source: T)
        where T: Source<Item = S> + Send + 'static
    {
        self.next_sounds.lock().unwrap().push((Box::new(source) as Box<_>, None));
    }

    /// Adds a new source to the end of the queue.
    ///
    /// The `Receiver` will be signalled when the sound has finished playing.
    #[inline]
    pub fn append_with_signal<T>(&self, source: T) -> Receiver<()>
        where T: Source<Item = S> + Send + 'static
    {
        let (tx, rx) = mpsc::channel();
        self.next_sounds.lock().unwrap().push((Box::new(source) as Box<_>, Some(tx)));
        rx
    }
}

/// The output of the queue. Implements `Source`.
pub struct SourcesQueueOutput<S> {
    // The current iterator that produces samples.
    current: Box<Source<Item = S> + Send>,

    // Signal this sender before picking from `next`.
    signal_after_end: Option<Sender<()>>,

    // The next sounds.
    input: Arc<SourcesQueueInput<S>>,

    // See constructor.
    keep_alive_if_empty: bool,
}

impl<S> Source for SourcesQueueOutput<S> where S: Sample + Send + 'static {
    #[inline]
    fn get_current_frame_len(&self) -> Option<usize> {
        self.current.get_current_frame_len()
    }

    #[inline]
    fn get_channels(&self) -> u16 {
        self.current.get_channels()
    }

    #[inline]
    fn get_samples_rate(&self) -> u32 {
        self.current.get_samples_rate()
    }

    #[inline]
    fn get_total_duration(&self) -> Option<Duration> {
        None
    }
}

impl<S> Iterator for SourcesQueueOutput<S> where S: Sample + Send + 'static {
    type Item = S;

    #[inline]
    fn next(&mut self) -> Option<S> {
        loop {
            // Basic situation that will happen most of the time.
            if let Some(sample) = self.current.next() {
                return Some(sample);
            }

            // Since `self.current` has finished, we need to pick the next sound.
            // In order to avoid inlining this expensive operation, the code is in another function.
            if self.go_next().is_err() {
                return None;
            }
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.current.size_hint().0, None)
    }
}

impl<S> SourcesQueueOutput<S> where S: Sample + Send + 'static {
    // Called when `current` is empty and we must jump to the next element.
    // Returns `Ok` if the sound should continue playing, or an error if it should stop.
    //
    // This method is separate so that it is not inlined.
    fn go_next(&mut self) -> Result<(), ()> {
        if let Some(signal_after_end) = self.signal_after_end.take() {
            let _ = signal_after_end.send(());
        }

        let (next, signal_after_end) = {
            let mut next = self.input.next_sounds.lock().unwrap();

            if next.len() == 0 {
                if self.keep_alive_if_empty {
                    // Play a short silence in order to avoid spinlocking.
                    let silence = Zero::<S>::new(1, 44000);          // TODO: meh
                    (Box::new(silence.take_duration(Duration::from_millis(10))) as Box<_>, None)
                } else {
                    return Err(());
                }
            } else {
                next.remove(0)
            }
        };

        self.current = next;
        self.signal_after_end = signal_after_end;
        Ok(())
    }
}
