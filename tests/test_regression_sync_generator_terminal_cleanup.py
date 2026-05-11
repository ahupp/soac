def test_sync_generator_terminal_cleanup_releases_state_before_wrapper_destruction(
    run_integration_module,
):
    with run_integration_module("sync_generator_terminal_cleanup") as module:
        assert module.completed_payload_released() == (None, None)
        assert module.escaped_payload_released() == (None, None)
        assert module.closed_throw_uses_terminal_state() == ("boom", None)
