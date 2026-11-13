/**
 * Owner: onboarding. Ghost's ears and voice (ADR 0066): Parakeet and Kokoro are not in the
 * bundle, so the app downloads them, here, with progress and a failure said in plain words.
 * The daemon does the work (`POST /speech-models/fetch`), each file checked against its pin;
 * this only asks and shows. Read again every second while a download runs, every five while
 * something is missing, and not at all once everything is here.
 */
import type { GhostClient, SpeechModelsView } from "@ghost/client-ts";
import { useCallback, useEffect, useState } from "react";

import { Badge, Button, errorText, ListRow, Progress, Skeleton } from "../../ui";

export const gb = (bytes: number) => `${(bytes / 1e9).toFixed(1)} GB`;

/** What the panel says, from the view alone (tested on its own). */
export function speechLine(v: SpeechModelsView | null): { state: "unknown" | "here" | "missing" | "fetching" | "failed" | "loading"; text: string } {
  if (!v) return { state: "unknown", text: "Checking what this computer has." };
  if (v.fetch.state === "fetching") {
    const of = `${gb(v.fetch.done_bytes)} of ${gb(v.fetch.total_bytes)}`;
    return { state: "fetching", text: `Downloading ${of}${v.fetch.file ? ` · ${v.fetch.file}` : ""}` };
  }
  if (v.missing.length === 0) {
    return v.ready ? { state: "here", text: "Ghost’s ears and voice are on this computer. Speech never needs the internet." } : { state: "loading", text: "Every file is here. Ghost is loading them." };
  }
  if (v.fetch.state === "failed") return { state: "failed", text: v.fetch.error ?? "The download stopped." };
  const short = v.free_bytes !== null && v.free_bytes < v.missing_bytes ? ` There is ${gb(v.free_bytes)} free there; it needs ${gb(v.missing_bytes)}.` : "";
  return { state: "missing", text: `Ghost’s ears (Parakeet) and voice (Kokoro) are ${gb(v.missing_bytes)} to download, once. After that speech never needs the internet.${short}` };
}

export function useSpeechModels(client: GhostClient, online: boolean) {
  const [view, setView] = useState<SpeechModelsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const read = useCallback(() => client.speechModels().then(setView, () => undefined), [client]);
  useEffect(() => {
    if (!online) return;
    void read();
  }, [online, read]);
  const fetching = view?.fetch.state === "fetching";
  const settled = !!view && view.missing.length === 0 && view.ready;
  useEffect(() => {
    if (!online || settled) return;
    const t = setInterval(() => void read(), fetching ? 1000 : 5000);
    return () => clearInterval(t);
  }, [online, settled, fetching, read]);
  const fetch = useCallback(async () => {
    setError(null);
    try {
      setView(await client.fetchSpeechModels());
    } catch (e) {
      setError(errorText(e));
    }
  }, [client]);
  return { view, fetch, error };
}

/** One row: what is here, the download, or why it stopped. */
export function SpeechModelsRow({ client, online }: { client: GhostClient; online: boolean }) {
  const { view, fetch, error } = useSpeechModels(client, online);
  const line = speechLine(view);
  const share = view && view.fetch.total_bytes > 0 ? view.fetch.done_bytes / view.fetch.total_bytes : 0;
  return (
    <ListRow
      title="Ghost’s ears and voice"
      description={
        <>
          {error ?? line.text}
          {line.state === "fetching" && <Progress value={share * 100} label="Downloading the speech models" />}
        </>
      }
      trailing={
        line.state === "here" ? (
          <Badge tone="ok" dot>
            Here
          </Badge>
        ) : line.state === "loading" ? (
          <Badge tone="info">Loading</Badge>
        ) : line.state === "fetching" ? (
          <Badge tone="info">{Math.floor(share * 100)}%</Badge>
        ) : line.state === "missing" || line.state === "failed" ? (
          <Button size="sm" disabled={!online} onClick={() => void fetch()}>
            {line.state === "failed" ? "Try again" : "Download"}
          </Button>
        ) : (
          <Skeleton width={80} height={20} />
        )
      }
    />
  );
}
