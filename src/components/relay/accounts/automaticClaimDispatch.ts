/** An original transaction owner proved that it did not dispatch a policy write. */
export class AutomaticClaimNotDispatchedError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "AutomaticClaimNotDispatchedError";
  }
}
