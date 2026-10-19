"""``Status.ending``: set when Ghost hangs up, cleared when the session is over."""

from __future__ import annotations

from ghost_api import DaemonState, Phase


def test_ending_is_set_in_a_session_and_cleared_after() -> None:
    st = DaemonState(version="test", wake_mode="file")
    st.set_ending()
    assert st.ending is False  # no session, nothing to hang up
    st.set_phase(Phase.LISTENING, "s1", origin="phone")
    st.set_ending()
    assert st.ending is True
    st.set_phase(Phase.PASSIVE)
    assert st.ending is False
