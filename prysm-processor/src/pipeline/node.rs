use std::fmt::Debug;

/// Pipeline node that transforms input to output
pub trait Node<In, Out>: Debug + Send {
    /// Process input and produce output
    fn process(&mut self, input: In) -> Out;
}
