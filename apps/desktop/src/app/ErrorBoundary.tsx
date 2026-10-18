import { RotateCcw, TriangleAlert } from "lucide-react";
import { Component, type ErrorInfo, type ReactNode } from "react";

import { Button, EmptyState } from "../ui";

interface Props {
  /** Shown in the message: which screen failed. */
  name: string;
  children: ReactNode;
}
interface State {
  error: Error | null;
}

/** One per route: a screen that throws shows this, and the rest of the app keeps working. */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[ghost] ${this.props.name} crashed`, error, info.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    const chunk = /dynamically imported module|Failed to fetch/i.test(this.state.error.message);
    return (
      <EmptyState
        size="page"
        icon={<TriangleAlert />}
        title={chunk ? `${this.props.name} didn’t load` : `${this.props.name} hit a problem`}
        description={
          <>
            The rest of Ghost is fine. {chunk ? "Reload the window to fetch it again." : "Try again, and if it keeps happening the message below says why."}
            <code className="shell-error-detail">{this.state.error.message}</code>
          </>
        }
        actions={
          <Button leadingIcon={<RotateCcw />} onClick={() => (chunk ? window.location.reload() : this.setState({ error: null }))}>
            {chunk ? "Reload" : "Try again"}
          </Button>
        }
      />
    );
  }
}
