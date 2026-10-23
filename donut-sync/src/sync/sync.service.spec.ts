import { ForbiddenException } from "@nestjs/common";
import type { ConfigService } from "@nestjs/config";
import { SyncService } from "./sync.service.js";

function createService() {
  const values: Record<string, string> = {
    S3_ENDPOINT: "http://127.0.0.1:9000",
    S3_ACCESS_KEY_ID: "test",
    S3_SECRET_ACCESS_KEY: "test",
    S3_BUCKET: "test",
  };
  const config = {
    get: jest.fn((key: string) => values[key]),
  } as unknown as ConfigService;
  return new SyncService(config);
}

describe("SyncService.deletePrefix", () => {
  it("rejects an empty self-hosted prefix before calling S3", async () => {
    const service = createService();
    const send = jest.fn();
    Object.assign(service, { s3Client: { send } });

    await expect(
      service.deletePrefix(
        { prefix: "" },
        { mode: "self-hosted", prefix: "", profileLimit: 0 },
      ),
    ).rejects.toBeInstanceOf(ForbiddenException);
    expect(send).not.toHaveBeenCalled();
  });
});
