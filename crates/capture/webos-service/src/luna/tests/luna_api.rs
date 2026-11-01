impl Luna {
    pub(crate) fn test_api() -> Self {
        use crate::luna::tests as mock;
        Self {
            __library: mock::process_library(),
            LSErrorInit: mock::error_init,
            LSErrorFree: mock::error_free,
            LSRegister: mock::register,
            LSRegisterCategory: mock::category,
            LSCategorySetData: mock::category_data,
            LSUnregister: mock::unregister,
            LSMessageGetPayload: mock::payload,
            LSMessageReply: mock::reply,
            LSGmainAttach: mock::loop_attach,
            LSCall: mock::call,
            LSCallCancel: mock::cancel,
            LSSubscriptionProcess: mock::subscribe,
            LSSubscriptionReply: mock::publish,
            LSSubscriptionGetHandleSubscribersCount: mock::subscribers,
        }
    }
}
