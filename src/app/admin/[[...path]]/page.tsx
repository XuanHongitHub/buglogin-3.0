import { WebRouteHost } from "../../web-route-host";

export const dynamicParams = false;

const routes = [
  "overview",
  "users",
  "organizations",
  "workspaces",
  "jobs",
  "incidents",
  "audit",
];

export function generateStaticParams() {
  return [[], ...routes.map((route) => [route])].map((path) => ({ path }));
}

export default async function AdminRoute({ params }: { params: Promise<{ path?: string[] }> }) {
  const { path = [] } = await params;
  return <WebRouteHost initialPath={path.length ? `/admin/${path.join("/")}` : "/admin"} />;
}
