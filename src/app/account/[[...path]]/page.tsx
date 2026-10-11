import { WebRouteHost } from "../../web-route-host";

export const dynamicParams = false;

export function generateStaticParams() {
  return [[], ["profile"], ["security"], ["sessions"], ["tokens"]].map(
    (path) => ({ path }),
  );
}

export default async function AccountRoute({ params }: { params: Promise<{ path?: string[] }> }) {
  const { path = [] } = await params;
  return <WebRouteHost initialPath={path.length ? `/account/${path.join("/")}` : "/account"} />;
}
