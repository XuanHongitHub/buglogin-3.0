import type { ImgHTMLAttributes } from "react";

type NextImageProps = ImgHTMLAttributes<HTMLImageElement> & {
  src: string;
  alt: string;
  width?: number;
  height?: number;
  fill?: boolean;
  priority?: boolean;
  unoptimized?: boolean;
};

export default function NextImage({
  fill,
  priority,
  unoptimized,
  alt,
  ...props
}: NextImageProps) {
  return (
    <img
      alt={alt}
      {...props}
      style={
        fill
          ? {
              ...props.style,
              position: "absolute",
              inset: 0,
              width: "100%",
              height: "100%",
            }
          : props.style
      }
    />
  );
}
