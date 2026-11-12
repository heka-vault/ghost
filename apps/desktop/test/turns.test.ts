import { describe, expect, it } from "vitest";

import { type Captions, captionPhase, captionTurn, joinSpoken, noCaptions, withoutHeld } from "@ghost/ui";

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

describe("captions: the operator's words show once, whole, when the turn closes", () => {
  type T = { role: string; text: string; via?: string; at: string };
  const play = (steps: (T | string)[]): Captions<T> =>
    steps.reduce<Captions<T>>((c, s) => (typeof s === "string" ? captionPhase(c, s) : captionTurn(c, s)), noCaptions<T>());
  const shown = (c: Captions<T>) => joinSpoken(c.shown).map((t) => t.text);

  it("holds a sentence's pieces while the operator is still talking", () => {
    const c = play([said("Can you check on the deploy", 1), "listening", said("for the billing service", 3)]);
    expect(c.shown).toEqual([]);
    expect(c.held).toHaveLength(2);
  });

  it("shows the pieces together, once, when the turn closes", () => {
    const c = play([said("Can you check on the deploy", 1), said("for the billing service", 3), said("and tell me if it finished.", 5), "thinking"]);
    expect(shown(c)).toEqual(["Can you check on the deploy for the billing service and tell me if it finished."]);
    expect(c.held).toEqual([]);
  });

  it("shows them when Ghost starts to answer, if that is the first sign the turn closed", () => {
    expect(shown(play([said("What's waiting?", 1), "speaking"]))).toEqual(["What's waiting?"]);
    expect(shown(play([said("What's waiting?", 1), reply("Two issues.", 3)]))).toEqual(["What's waiting?", "Two issues."]);
  });

  it("never holds a typed turn, and a typed turn brings the held words with it", () => {
    expect(shown(play([typed("close infra 17", 1)]))).toEqual(["close infra 17"]);
    expect(play([said("Actually", 1), typed("close infra 17", 2)]).shown.map((t) => t.text)).toEqual(["Actually", "close infra 17"]);
  });

  it("holds words said over Ghost until that turn closes too", () => {
    const c = play([said("Read me the first one.", 1), "thinking", reply("Issue 415 is", 3), "speaking", "listening", said("No, the second", 5)]);
    expect(shown(c)).toEqual(["Read me the first one.", "Issue 415 is"]);
    expect(shown(captionPhase(c, "thinking"))).toEqual(["Read me the first one.", "Issue 415 is", "No, the second"]);
  });

  it("drops everything, held or not, when the session ends", () => {
    expect(play([said("Thanks", 1), reply("Bye.", 2), said("oh and", 3), "passive"])).toEqual({ shown: [], held: [] });
  });

  it("keeps the last 200 shown", () => {
    const many = Array.from({ length: 210 }, (_, i) => reply(`r${i}`, i % 60));
    expect(play(many).shown).toHaveLength(200);
  });
});

describe("withoutHeld(): a stored transcript re-read mid-sentence matches the live view", () => {
  const stored = (role: string, text: string) => ({ role, text });
  it("drops the held pieces from the end, and nothing before Ghost's last reply", () => {
    const turns = [stored("user", "What's waiting?"), stored("assistant", "Two issues."), stored("user", "Read me"), stored("user", "the first")];
    expect(withoutHeld(turns, 2)).toEqual(turns.slice(0, 2));
    expect(withoutHeld(turns, 5)).toEqual(turns.slice(0, 2));
    expect(withoutHeld(turns, 0)).toEqual(turns);
  });
});
