import { describe, expect, it } from "vitest";

import { joinSpoken } from "@ghost/ui";

const at = (s: number) => `2026-10-29T23:20:${String(s).padStart(2, "0")}Z`;
const said = (text: string, s: number) => ({ role: "user", text, via: "voice", at: at(s) });
const typed = (text: string, s: number) => ({ role: "user", text, via: "text", at: at(s) });
const reply = (text: string, s: number) => ({ role: "assistant", text, via: "voice", at: at(s) });

describe("joinSpoken(): one message per thing the operator said", () => {
  it("joins a sentence heard in pieces, as heard, at its first piece's time", () => {
    const turns = [said("Tell me a little bit more about", 1), said("issue four one five.", 3), reply("It's an enhancement.", 6)];
    expect(joinSpoken(turns)).toEqual([{ ...said("Tell me a little bit more about issue four one five.", 1) }, reply("It's an enhancement.", 6)]);
  });

  it("keeps what Ghost answered between them apart", () => {
    const turns = [said("What's waiting?", 1), reply("Two issues.", 3), said("Thanks.", 5)];
    expect(joinSpoken(turns)).toEqual(turns);
  });

  it("keeps typed messages one each, and apart from spoken ones", () => {
    const turns = [typed("close infra 17", 1), typed("and 18", 2), said("Actually wait.", 3)];
    expect(joinSpoken(turns)).toEqual(turns);
  });

  it("joins stored turns, which don't say how they came", () => {
    const turns = [
      { id: 1, role: "user", text: "Can you count", at: at(1) },
      { id: 2, role: "user", text: "to three", at: at(2) },
      { id: 3, role: "call", text: "count()", at: at(3) },
      { id: 4, role: "user", text: "Thanks.", at: at(4) },
    ];
    expect(joinSpoken(turns).map((t) => [t.id, t.text])).toEqual([
      [1, "Can you count to three"],
      [3, "count()"],
      [4, "Thanks."],
    ]);
  });

  it("leaves the turns it was given alone", () => {
    const turns = [said("Can you count", 1), said("to three", 2)];
    joinSpoken(turns);
    expect(turns[0].text).toBe("Can you count");
  });
});
