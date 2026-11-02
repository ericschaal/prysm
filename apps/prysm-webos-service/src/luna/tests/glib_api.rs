impl Glib {
    pub(crate) fn test_api() -> Self {
        use crate::luna::tests as mock;
        Self {
            __library: mock::process_library(),
            g_main_context_new: mock::context_new,
            g_main_context_unref: mock::context_unref,
            g_main_loop_new: mock::loop_new,
            g_main_loop_run: mock::loop_run,
            g_main_loop_quit: mock::loop_quit,
            g_main_loop_unref: mock::loop_unref,
            g_source_unref: mock::source_unref,
            g_source_attach: mock::source_attach,
            g_source_destroy: mock::source_destroy,
            g_source_set_callback: mock::source_callback,
            g_timeout_source_new: mock::timer_new,
        }
    }
}
