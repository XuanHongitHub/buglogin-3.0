import type {
  Operation,
  OperationAccepted,
} from "../../apps/desktop/src/contracts";

const TERMINAL_STATUSES = new Set<Operation["status"]>([
  "completed",
  "failed",
  "cancelled",
]);

export async function awaitDesktopOperation(
  initial: OperationAccepted,
  getOperation: (operationId: string) => Promise<Operation>,
  options: { intervalMs?: number; maxAttempts?: number } = {},
): Promise<Operation> {
  const intervalMs = options.intervalMs ?? 500;
  const maxAttempts = options.maxAttempts ?? 600;
  let current: Operation = {
    operationId: initial.operationId,
    status: "queued",
  };

  for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
    current = await getOperation(initial.operationId);
    if (TERMINAL_STATUSES.has(current.status)) return current;
  }

  throw new Error(
    "Operation is still in progress. Refresh and inspect its status.",
  );
}
