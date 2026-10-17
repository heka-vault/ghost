import { phaseLabel } from "@ghost/ui";
import { useGhost } from "@ghost/ui";

export function StatusDot() {
  const { status, connected } = useGhost();
  const phase = connected && status ? status.phase : "off";
  const title = connected && status ? phaseLabel(status.phase) : "Not connected";
  return <span className={`dot ${phase}`} title={title} />;
}
