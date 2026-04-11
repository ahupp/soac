def test_wait_for_timeout_releases_payload(run_integration_module):
    with run_integration_module("asyncio_wait_for_release_regression") as module:
        assert module.throw_cancelled_check() is None
        assert module.leak_check() is None
