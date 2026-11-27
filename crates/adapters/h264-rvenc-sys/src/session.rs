//! Whether an encoder session may encode: any error ends it. Safe and
//! testable on any host.
use crate::Error;

/// A session open until its first error. After one, `rvenc` may still hold a
/// slot or emit a late access unit, so encoding again could hand picture N's
/// output to N+1 or overwrite a slot in use; only a new pipeline is safe.
#[derive(Debug, Default)]
pub(crate) struct Session {
    failed: bool,
}

impl Session {
    /// `Ok` while no call of this session has failed.
    pub(crate) fn open(&self) -> Result<(), Error> {
        if self.failed {
            Err(Error::Failed)
        } else {
            Ok(())
        }
    }

    /// `result` of one call, which ends the session if it is an error.
    pub(crate) fn record<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        self.failed |= result.is_err();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_ends_the_session() {
        let mut session = Session::default();
        session.open().unwrap();
        assert_eq!(session.record(Ok(1)).unwrap(), 1);
        session.open().unwrap();
        assert!(matches!(
            session.record::<()>(Err(Error::Timeout)),
            Err(Error::Timeout)
        ));
        assert!(matches!(session.open(), Err(Error::Failed)));
        session.record(Ok(())).unwrap();
        assert!(matches!(session.open(), Err(Error::Failed)));
    }
}
