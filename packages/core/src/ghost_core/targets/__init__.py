"""Generic target/writer pairs so simple abilities don't write their own."""

from ghost_core.targets.files import AppendText, FileSnapshot, FileTarget, FileWriter

__all__ = ["AppendText", "FileSnapshot", "FileTarget", "FileWriter"]
