import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function Icon({ size = 16, children, ...rest }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...rest}
    >
      {children}
    </svg>
  );
}

export const CubeIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z" />
    <path d="M12 12l8-4.5M12 12v9M12 12L4 7.5" />
  </Icon>
);

export const CameraIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M4 8.5A2.5 2.5 0 016.5 6h5A2.5 2.5 0 0114 8.5v7A2.5 2.5 0 0111.5 18h-5A2.5 2.5 0 014 15.5v-7z" />
    <path d="M14 11l6-3.5v9L14 13z" />
  </Icon>
);

export const SunIcon = (props: IconProps) => (
  <Icon {...props}>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
  </Icon>
);

export const EyeIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" />
    <circle cx="12" cy="12" r="3" />
  </Icon>
);

export const EyeOffIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M3 3l18 18" />
    <path d="M10.6 6.1A9.9 9.9 0 0112 6c6 0 9.5 6 9.5 6a17.6 17.6 0 01-3.5 4" />
    <path d="M6.3 7.8A17.4 17.4 0 002.5 12S6 18 12 18a9.6 9.6 0 003.5-.7" />
    <path d="M9.9 9.9a3 3 0 004.2 4.2" />
  </Icon>
);

export const LockIcon = (props: IconProps) => (
  <Icon {...props}>
    <rect x="5" y="10.5" width="14" height="10" rx="2" />
    <path d="M8.5 10.5V7.5a3.5 3.5 0 017 0v3" />
  </Icon>
);

export const UnlockIcon = (props: IconProps) => (
  <Icon {...props}>
    <rect x="5" y="10.5" width="14" height="10" rx="2" />
    <path d="M8.5 10.5V7.5a3.5 3.5 0 016.8-1.2" />
  </Icon>
);

export const UndoIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M9 7H5V3" />
    <path d="M5 7l3.5-3.5" />
    <path d="M5.5 7H14a5.5 5.5 0 110 11H8" />
  </Icon>
);

export const RedoIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M15 7h4V3" />
    <path d="M19 7l-3.5-3.5" />
    <path d="M18.5 7H10a5.5 5.5 0 100 11h6" />
  </Icon>
);

export const SaveIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M5 3h11l3 3v15H5z" />
    <path d="M8 3v6h8V3M8 21v-7h8v7" />
  </Icon>
);

export const FolderIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M3 7a2 2 0 012-2h4l2 2.5h8a2 2 0 012 2V18a2 2 0 01-2 2H5a2 2 0 01-2-2z" />
  </Icon>
);

export const FileIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M6 3h8l4 4v14H6z" />
    <path d="M14 3v4h4" />
  </Icon>
);

export const PlusIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M12 5v14M5 12h14" />
  </Icon>
);

export const FrameIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M4 9V5h4M20 9V5h-4M4 15v4h4M20 15v4h-4" />
  </Icon>
);

export const GridIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M3 9h18M3 15h18M9 3v18M15 3v18" />
  </Icon>
);

export const AlertIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M12 4l9 16H3z" />
    <path d="M12 10v4M12 17.2v.1" />
  </Icon>
);

export const CloseIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M6 6l12 12M18 6L6 18" />
  </Icon>
);

export const RestoreIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M4 12a8 8 0 108-8" />
    <path d="M4 4v5h5" />
  </Icon>
);

export const objectIcon = (type: "mesh" | "camera" | "light") => {
  switch (type) {
    case "mesh":
      return CubeIcon;
    case "camera":
      return CameraIcon;
    case "light":
      return SunIcon;
  }
};
