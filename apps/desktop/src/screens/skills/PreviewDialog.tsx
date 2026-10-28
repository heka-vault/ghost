/**
 * Owner: skills. Look at a skill before installing it (#35): Ghost fetches its folder from
 * GitHub, pinned to a commit, and this shows the SKILL.md as the source wrote it and every
 * script in the folder. All of it is the source's text: shown as plain text, never acted
 * on. Install copies that very folder into Ghost's skills through the gate.
 *
 * Three ways in: a skills.sh result, an address the operator types (owner/repo/skill or a
 * github.com link), or an update to a skill Ghost installed from GitHub.
 */
import type { SkillInfo, SkillListing, SkillPreview } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useCallback, useEffect, useRef, useState } from "react";

import { Button, Card, Dialog, Field, Input, Notice, SkeletonText, errorText, useLive } from "../../ui";
import { Block } from "./Block";
import { humanize, saysInstalled, sentence, shortCommit, sizeWords } from "./model";

export type PreviewStart = { mode: "address" } | { mode: "listing"; listing: SkillListing } | { mode: "update"; name: string; from: string };

export interface PreviewDialogProps {
  start: PreviewStart | null;
  onClose: () => void;
  onInstalled: (info: SkillInfo, updated: boolean) => void;
}

const FORM_ID = "skl-address-form";

export function PreviewDialog({ start, onClose, onInstalled }: PreviewDialogProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const [address, setAddress] = useState("");
  const [preview, setPreview] = useState<SkillPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const seq = useRef(0);
  const input = useRef<HTMLInputElement>(null);

  const look = useCallback(
    async (s: PreviewStart, typed = "") => {
      const n = ++seq.current;
      setLoading(true);
      setFetchError(null);
      setInstallError(null);
      setPreview(null);
      try {
        const p =
          s.mode === "update" ? await client.previewSkillUpdate(s.name)
          : s.mode === "listing" ? await client.previewSkill(s.listing.source, s.listing.skill)
          : await client.previewSkill(typed);
        if (n !== seq.current) return;
        setPreview(p);
        setName(s.mode === "update" ? s.name : p.name);
      } catch (e) {
        if (n === seq.current) setFetchError(errorText(e));
      } finally {
        if (n === seq.current) setLoading(false);
      }
    },
    [client],
  );

  useEffect(() => {
    seq.current++;
    setPreview(null);
    setFetchError(null);
    setInstallError(null);
    setLoading(false);
    setAddress("");
    if (start && start.mode !== "address") void look(start);
  }, [start, look]);

  const update = start?.mode === "update" ? start : null;
  const theirs = preview?.installed === "home" && !update;
  const replaces = !!preview && (!!update || (preview.installed === "ghost" && name === preview.name));
  const nameTaken = theirs && name.trim().toLowerCase() === preview?.name;

  const install = async () => {
    if (!preview) return;
    setInstalling(true);
    setInstallError(null);
    try {
      const info = await client.installSkill({ token: preview.token, name: name.trim() || null, replace: replaces });
      onInstalled(info, !!update);
      onClose();
    } catch (e) {
      const text = errorText(e);
      // A spent or expired preview can't be installed; fetch it again rather than leave a dead button.
      setInstallError(/preview is gone/i.test(text) ? "This look at the skill expired. Look again to install it." : text);
    } finally {
      setInstalling(false);
    }
  };

  const title =
    update ? `Update ${humanize(update.name)}`
    : preview ? `Install ${preview.name}`
    : start?.mode === "listing" ? `Install ${start.listing.name}`
    : "Install from GitHub";
  const addressStep = start?.mode === "address" && !preview;
  const primary =
    addressStep ? (
      <Button variant="primary" type="submit" form={FORM_ID} loading={loading} disabled={!online}>
        Look at it
      </Button>
    ) : preview ? (
      <Button variant="primary" loading={installing} disabled={!online || nameTaken || !name.trim()} onClick={() => void install()}>
        {update ? "Update" : replaces ? `Replace ${humanize(name)}` : "Install"}
      </Button>
    ) : null;

  return (
    <Dialog
      open={!!start}
      onClose={onClose}
      dismissible={!installing}
      size="lg"
      initialFocus={addressStep ? input : undefined}
      title={title}
      description={
        addressStep
          ? "A skill on GitHub: owner/repo/skill, or the address of its folder. Ghost fetches it and shows it to you before anything is installed."
          : "Read it before you install it. This is the source’s own text: it shapes what Ghost does, never what it may do, and nothing in it runs when you install."
      }
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={installing}>
            Cancel
          </Button>
          {primary}
        </>
      }
    >
      {addressStep && (
        <form
          id={FORM_ID}
          onSubmit={(e) => {
            e.preventDefault();
            if (address.trim()) void look({ mode: "address" }, address.trim());
            else input.current?.focus();
          }}
        >
          <Field label="GitHub address" help="Like anthropics/skills/pdf or https://github.com/anthropics/skills/tree/main/skills/pdf.">
            <Input
              ref={input}
              mono
              value={address}
              placeholder="anthropics/skills/pdf"
              autoComplete="off"
              spellCheck={false}
              disabled={loading}
              onChange={(e) => {
                setAddress(e.target.value);
                setFetchError(null);
              }}
            />
          </Field>
        </form>
      )}

      {fetchError && (
        <Notice
          tone="danger"
          title="Couldn’t fetch that skill"
          className="skl-preview__notice"
          action={
            start && start.mode !== "address" ? (
              <Button size="sm" variant="secondary" onClick={() => void look(start)}>
                Try again
              </Button>
            ) : undefined
          }
        >
          <p className="skl-notice__p" data-user-content>
            {sentence(fetchError)}
          </p>
        </Notice>
      )}

      {loading && !addressStep && (
        <div className="skl-preview__loading" aria-busy="true" aria-live="polite">
          <p className="t-small t-2">Fetching it from GitHub…</p>
          <SkeletonText lines={6} />
        </div>
      )}

      {preview && (
        <div className="skl-preview">
          <p className="skl-preview__meta">
            <span className="t-mono" data-user-content>
              {preview.source}
            </span>
            <span aria-hidden>·</span>
            <span className="t-mono" data-user-content>
              {preview.skill_path.replace(/\/?SKILL\.md$/, "") || "the repository’s top folder"}
            </span>
            <span aria-hidden>·</span>
            <span>
              {update ? (
                <>
                  from <span className="t-mono">{shortCommit(update.from)}</span> to <span className="t-mono">{shortCommit(preview.commit)}</span>
                </>
              ) : (
                <>
                  at <span className="t-mono">{shortCommit(preview.commit)}</span>
                </>
              )}
            </span>
          </p>
          {preview.description && (
            <p className="skl-preview__description" data-user-content>
              {preview.description}
            </p>
          )}

          {theirs && (
            <Notice tone="warn" title={`You already have a skill called ${preview.name}`} className="skl-preview__notice">
              <p className="skl-notice__p">It’s yours, in ~/.agents/skills, and Ghost won’t hide it. Install this one under another name.</p>
            </Notice>
          )}
          {preview.installed === "ghost" && !update && (
            <Notice tone="warn" title={`Ghost already has a skill called ${preview.name}`} className="skl-preview__notice">
              <p className="skl-notice__p">Installing replaces it, and the copy Ghost has is deleted. To keep both, install this one under another name.</p>
            </Notice>
          )}
          {update && (
            <Notice tone="warn" title="Your changes to it are replaced" className="skl-preview__notice">
              <p className="skl-notice__p">Updating replaces Ghost’s copy of {humanize(update.name)} with this one, whole. If you edited its files, those edits are gone. If the update fails, the copy you have stays as it is.</p>
            </Notice>
          )}
          {(theirs || (preview.installed === "ghost" && !update)) && (
            <div className="skl-preview__name">
              <Field label="Install as" help="Letters, digits, dots and dashes." error={nameTaken ? "That’s your skill’s name. Pick another." : undefined}>
                <Input mono value={name} autoComplete="off" spellCheck={false} onChange={(e) => setName(e.target.value)} />
              </Field>
            </div>
          )}

          <Block
            title="Scripts"
            count={preview.scripts.length}
            description={
              preview.scripts.length > 0
                ? "None of these runs when you install. Ghost runs one only through the Skill scripts app, as a change your permissions decide."
                : undefined
            }
          >
            {preview.scripts.length > 0 ? (
              <Card list>
                {preview.scripts.map((f) => (
                  <div key={f} className="skl-item skl-item--compact">
                    <p className="skl-item__title t-mono" data-user-content>
                      {f}
                    </p>
                  </div>
                ))}
              </Card>
            ) : (
              <p className="skl-none">No scripts: only text Ghost reads.</p>
            )}
          </Block>

          <Block title="Other files" count={preview.files.filter((f) => !f.script).length}>
            {preview.files.some((f) => !f.script) ? (
              <Card list>
                {preview.files
                  .filter((f) => !f.script)
                  .map((f) => (
                    <div key={f.path} className="skl-item skl-item--compact">
                      <p className="skl-item__title t-mono" data-user-content>
                        {f.path}
                      </p>
                      <span className="skl-item__size t-num">{sizeWords(f.bytes)}</span>
                    </div>
                  ))}
              </Card>
            ) : (
              <p className="skl-none">Only SKILL.md and the scripts.</p>
            )}
          </Block>

          <Block title="SKILL.md" description="The whole file, as the source wrote it.">
            <Card tone="sunken">
              <pre className="skl-body skl-preview__md" data-user-content tabIndex={0} aria-label="SKILL.md">
                {preview.skill_md}
              </pre>
            </Card>
          </Block>

          {installError && (
            <Notice
              tone="danger"
              title={update ? "Couldn’t update it" : "Couldn’t install it"}
              className="skl-preview__notice"
              action={
                /expired/.test(installError) && start ? (
                  <Button size="sm" variant="secondary" onClick={() => void look(start, address.trim())}>
                    Look again
                  </Button>
                ) : undefined
              }
            >
              <p className="skl-notice__p" data-user-content>
                {sentence(installError)}
              </p>
              {saysInstalled(installError) && <p className="skl-notice__p">Pick another name to keep both.</p>}
            </Notice>
          )}
        </div>
      )}
    </Dialog>
  );
}
