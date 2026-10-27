/** A refusal from the mock daemon, answered with the daemon's status and its own sentence. */
export class HttpError extends Error {
  constructor(public status: number, public detail: string) {
    super(detail);
  }
}
