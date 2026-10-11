import { WebRouteHost } from "../../web-route-host";

export const dynamicParams = true;

export function generateStaticParams() {
  return [{ path: [] }];
}

export default async function WorkspaceRoute({ params }: { params: Promise<{ path?: string[] }> }) {
  const { path = [] } = await params;
  return <WebRouteHost initialPath={path.length ? `/workspaces/${path.join("/")}` : "/workspaces"} />;
}
