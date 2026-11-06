/**
 * Owner: hear. A run's report (HearReport), drawn from its data, not its markdown: the meta
 * facts, the intro, then each section (level 2 a heading, 3 a subheading) with its parts in
 * order: text, facts, tables (a comparison's verdict colored; a row about a take can play it)
 * and the pass rule's verdict. Backticks in titles and text are code.
 */
import type { HearPart, HearReport, HearRow, HearTable } from "@ghost/client-ts";
import { Play } from "lucide-react";
import { Fragment } from "react";

import { Badge, IconButton, Table, cx } from "../../ui";
import { splitCode, takeOfKey } from "./model";

/** Text with its `code` spans. */
export function Coded({ text }: { text: string }) {
  return (
    <>
      {splitCode(text).map((p, i) => (p.code ? <code key={i} className="hear-code">{p.text}</code> : <Fragment key={i}>{p.text}</Fragment>))}
    </>
  );
}

const VERDICT_TONE = { better: "ok", same: "neutral", worse: "danger" } as const;

export function ReportView({ report, onPlay, canPlay }: { report: HearReport; onPlay: (take: string) => void; canPlay: boolean }) {
  return (
    <div className="hear-report">
      {report.meta.length > 0 && <Facts facts={report.meta} />}
      {report.intro && (
        <p className="t-body-lg t-2 hear-report__intro">
          <Coded text={report.intro} />
        </p>
      )}
      {report.sections.map((s, i) => {
        const H = s.level >= 3 ? "h4" : "h3";
        return (
          <section key={i} className={cx("hear-report__section", s.level >= 3 && "is-sub")}>
            <H className={s.level >= 3 ? "t-subheading" : "t-heading"}>
              <Coded text={s.title} />
            </H>
            {s.parts.map((p, j) => (
              <Part key={j} part={p} onPlay={onPlay} canPlay={canPlay} />
            ))}
          </section>
        );
      })}
    </div>
  );
}

function Facts({ facts }: { facts: { name: string; value: string }[] }) {
  return (
    <dl className="hear-facts">
      {facts.map((f) => (
        <div key={f.name} className="hear-facts__row">
          <dt className="t-small t-2">{f.name}</dt>
          <dd className="t-small">
            <Coded text={f.value} />
          </dd>
        </div>
      ))}
    </dl>
  );
}

function Part({ part, onPlay, canPlay }: { part: HearPart; onPlay: (take: string) => void; canPlay: boolean }) {
  switch (part.kind) {
    case "text":
      return (
        <p className="t-body hear-report__text">
          <Coded text={part.text} />
        </p>
      );
    case "facts":
      return <Facts facts={part.facts} />;
    case "pass":
      return (
        <div className={cx("hear-pass", part.pass ? "is-pass" : "is-fail")} role="status">
          <span className="hear-pass__badge">{part.pass ? "Pass" : "Fail"}</span>
          <p className="t-body-lg">
            <Coded text={part.text} />
          </p>
        </div>
      );
    case "table":
      return <ReportTable table={part} onPlay={onPlay} canPlay={canPlay} />;
  }
}

function ReportTable({ table, onPlay, canPlay }: { table: HearTable; onPlay: (take: string) => void; canPlay: boolean }) {
  const verdictCol = verdictColumn(table);
  const takes = table.rows.some((r) => takeOfKey(r.take));
  return (
    <div className="hear-report__table">
      <Table density="compact">
        <thead>
          <tr>
            {table.columns.map((c, i) => (
              <th key={i} className={cx(i > 0 && numeric(table, i) && "is-num")}>
                <Coded text={c} />
              </th>
            ))}
            {takes && (
              <th className="is-shrink">
                <span className="sr-only">Play</span>
              </th>
            )}
          </tr>
        </thead>
        <tbody>
          {table.rows.map((r, i) => (
            <Row key={i} row={r} table={table} verdictCol={verdictCol} takes={takes} onPlay={onPlay} canPlay={canPlay} />
          ))}
        </tbody>
      </Table>
    </div>
  );
}

function Row({ row, table, verdictCol, takes, onPlay, canPlay }: { row: HearRow; table: HearTable; verdictCol: number; takes: boolean; onPlay: (take: string) => void; canPlay: boolean }) {
  const take = takeOfKey(row.take);
  return (
    <tr>
      {row.cells.map((c, i) => (
        <td key={i} className={cx(i > 0 && numeric(table, i) && "is-num is-nowrap", i === 0 && take && "is-mono is-nowrap")}>
          {i === verdictCol && row.verdict ? (
            <Badge size="sm" tone={VERDICT_TONE[row.verdict]} dot>
              {c}
            </Badge>
          ) : (
            <Coded text={c} />
          )}
        </td>
      ))}
      {takes && (
        <td className="is-shrink">{take && <IconButton size="sm" variant="ghost" label={`Play ${take}`} icon={<Play />} onClick={() => onPlay(take)} disabled={!canPlay} />}</td>
      )}
    </tr>
  );
}

/** The column a row's verdict belongs in: the one named Verdict, or the last. -1 when no row has one. */
export function verdictColumn(t: HearTable): number {
  if (!t.rows.some((r) => r.verdict)) return -1;
  const named = t.columns.findIndex((c) => c.trim().toLowerCase() === "verdict");
  return named >= 0 ? named : t.columns.length - 1;
}

/** A column of numbers (percents, counts, seconds) is right-aligned. */
function numeric(t: HearTable, i: number): boolean {
  const cells = t.rows.map((r) => r.cells[i] ?? "").filter((c) => c && c !== "-" && c !== "–");
  return cells.length > 0 && cells.every((c) => /^[−+-]?\d/.test(c.trim()));
}
