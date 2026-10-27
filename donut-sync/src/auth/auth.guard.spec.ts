import { generateKeyPairSync } from "node:crypto";
import type { ExecutionContext } from "@nestjs/common";
import { UnauthorizedException } from "@nestjs/common";
import type { ConfigService } from "@nestjs/config";
import * as jwt from "jsonwebtoken";
import { AuthGuard } from "./auth.guard.js";

const issuer = "https://auth.buglogin.test";
const audience = "donut-sync";
const { privateKey, publicKey } = generateKeyPairSync("rsa", {
  modulusLength: 2048,
});

function config(values: Record<string, string | undefined>): ConfigService {
  return {
    get: jest.fn((key: string) => values[key]),
  } as unknown as ConfigService;
}

function context(token: string): {
  context: ExecutionContext;
  request: Record<string, unknown>;
} {
  const request = { headers: { authorization: `Bearer ${token}` } };
  return {
    request,
    context: {
      switchToHttp: () => ({ getRequest: () => request }),
    } as unknown as ExecutionContext,
  };
}

function sign(payload: jwt.JwtPayload, options: jwt.SignOptions = {}): string {
  return jwt.sign(payload, privateKey, {
    algorithm: "RS256",
    issuer,
    audience,
    subject: "user-1",
    expiresIn: "5m",
    ...options,
  });
}

function signWithoutClaim(
  payload: jwt.JwtPayload,
  claim: "sub" | "exp",
): string {
  const options: jwt.SignOptions = { algorithm: "RS256", issuer, audience };
  if (claim !== "sub") options.subject = "user-1";
  if (claim !== "exp") options.expiresIn = "5m";
  return jwt.sign(payload, privateKey, options);
}

describe("AuthGuard JWT claims", () => {
  const createGuard = () =>
    new AuthGuard(
      config({
        SYNC_JWT_PUBLIC_KEY: publicKey
          .export({ type: "spki", format: "pem" })
          .toString(),
        SYNC_JWT_ISSUER: issuer,
        SYNC_JWT_AUDIENCE: audience,
      }),
    );

  it("accepts a JWT with the required sync claims", async () => {
    const guard = createGuard();
    const { context: executionContext, request } = context(
      sign({ sync: true }),
    );

    await expect(guard.canActivate(executionContext)).resolves.toBe(true);
    expect(request.user).toEqual({
      mode: "cloud",
      prefix: "users/user-1/",
      profileLimit: 0,
      sub: "user-1",
    });
  });

  it.each([
    ["issuer", { sync: true }, { issuer: "https://wrong.test" }],
    ["audience", { sync: true }, { audience: "wrong" }],
    ["sub", { sync: true }, { subject: undefined }],
    ["exp", { sync: true }, { expiresIn: undefined }],
    ["iat", { sync: true }, { noTimestamp: true }],
    ["sync", {}, {}],
  ] satisfies Array<
    [string, jwt.JwtPayload, jwt.SignOptions]
  >)("rejects a JWT without a valid %s claim", async (_claim, payload, options) => {
    const guard = createGuard();
    const token =
      _claim === "sub" || _claim === "exp"
        ? signWithoutClaim(payload, _claim)
        : sign(payload, options);
    const { context: executionContext } = context(token);

    await expect(guard.canActivate(executionContext)).rejects.toBeInstanceOf(
      UnauthorizedException,
    );
  });

  it("fails closed when issuer or audience config is missing", () => {
    expect(
      () =>
        new AuthGuard(
          config({
            SYNC_JWT_PUBLIC_KEY: publicKey
              .export({ type: "spki", format: "pem" })
              .toString(),
          }),
        ),
    ).toThrow("SYNC_JWT_ISSUER and SYNC_JWT_AUDIENCE");
  });
});
