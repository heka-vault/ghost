"""Speech, not Markdown: what the TTS filter lets through."""

from ghost_voice.spoken import spoken


def test_drops_lines_with_nothing_to_say():
    assert spoken("---") == ""
    assert spoken("***\n") == ""
    assert spoken("| --- | --- |") == ""


def test_unwraps_links_emphasis_and_code():
    text = "**Your usage is high**. See [the plan](https://example.com/billing) and `needs-triage`."
    assert spoken(text) == "Your usage is high. See the plan and needs-triage."


def test_strips_list_heading_and_quote_markers():
    assert spoken("## Two apps\n- Subway\n- lululemon\n1. first\n> said") == (
        "Two apps\nSubway\nlululemon\nfirst\nsaid"
    )


def test_leaves_plain_speech_and_arithmetic_alone():
    assert spoken("Subway and lululemon both have one. ") == "Subway and lululemon both have one. "
    assert spoken("3 * 4 is 12, minus 2 - 1.") == "3 * 4 is 12, minus 2 - 1."
    assert spoken("") == ""


def test_the_notice_that_broke_kokoro():
    reply = (
        "Subway and lululemon both have one.\n\n---\n\n**Your Mobbin AI usage is high this "
        "month**\n\nUsage is unlimited during beta. [View usage](https://mobbin.com/x)"
    )
    assert spoken(reply) == (
        "Subway and lululemon both have one.\nYour Mobbin AI usage is high this month\n"
        "Usage is unlimited during beta. View usage"
    )


def test_emphasis_markers_only_at_word_edges():
    assert spoken("x**2 + y**2 = z**2") == "x**2 + y**2 = z**2"
    assert spoken("a_b__c__d") == "a_b__c__d"
    assert spoken("**Bold** start and __under__ score.") == "Bold start and under score."
