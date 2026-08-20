"""Public exceptions for the authenticated strict-language runtime.

Strict mode is selected by source and the interpreter's startup configuration,
not by importing this module or decorating individual classes.
"""

from _soac_ext import StrictMutationError, StrictRuntimeUnavailableError

__all__ = ["StrictMutationError", "StrictRuntimeUnavailableError"]
