import Image from "next/image";

type LogoProps = Omit<React.ComponentProps<typeof Image>, "src" | "alt">;

export const Logo = (props: LogoProps) => (
  <Image {...props} src="/buglogin-logo.png" alt="BugLogin logo" width={512} height={512} />
);
