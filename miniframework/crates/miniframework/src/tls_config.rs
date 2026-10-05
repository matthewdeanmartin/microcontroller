//! TLS configuration fallback independent of the ESP-IDF ABI.

/// An optional SDK initializer may leave unusable output on failure (IDF 5.5.3
/// ticket setup leaves a freed pointer). Publish a fresh default in that case.
/// Set certificates and ALPN only after this call.
pub(crate) fn optional_init<C: Default, E>(
    initialize: impl FnOnce(&mut C) -> Result<(), E>,
) -> (C, Option<E>) {
    let mut config = C::default();
    match initialize(&mut config) {
        Ok(()) => (config, None),
        Err(error) => (C::default(), Some(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Config {
        ticket_context: usize,
    }

    #[test]
    fn allocation_and_post_allocation_failure_never_publish_a_stale_context() {
        for fail_after_allocation in [false, true] {
            let mut freed = 0;
            let (config, error) = optional_init::<Config, _>(|config| {
                if fail_after_allocation {
                    config.ticket_context = 123;
                    freed += 1; // SDK freed it but failed to clear its out-parameter.
                }
                Err("ticket setup failed")
            });
            assert_eq!(error, Some("ticket setup failed"));
            assert_eq!(config.ticket_context, 0);
            assert_eq!(freed, usize::from(fail_after_allocation));
        }
        let (config, error) = optional_init::<Config, ()>(|config| {
            config.ticket_context = 123;
            Ok(())
        });
        assert_eq!(config.ticket_context, 123);
        assert_eq!(error, None);
    }
}
