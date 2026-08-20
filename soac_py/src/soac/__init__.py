"""Python support package for the SOAC transformed runtime."""

try:
    import _soac_ext as _soac_ext
except Exception as err:
    err.add_note(
        "soac requires the native extension '_soac_ext'; "
        "run 'just build-all' or 'just build-extension <debug|release>'"
    )
    raise

# The native exceptions use this module in their qualified names. Keep these
# aliases identical to soac.strict's exports, including for pickle/import.
StrictMutationError = _soac_ext.StrictMutationError
StrictRuntimeUnavailableError = _soac_ext.StrictRuntimeUnavailableError

__all__ = ["_soac_ext", "StrictMutationError", "StrictRuntimeUnavailableError"]
