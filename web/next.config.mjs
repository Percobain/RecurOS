/** @type {import('next').NextConfig} */
const RAW = "https://raw.githubusercontent.com/Percobain/RecurOS/main";

const nextConfig = {
  // Short, memorable install URLs that stay honest: they redirect to the
  // scripts in the repository, so there is only ever one copy to keep right.
  async redirects() {
    return [
      { source: "/install.sh", destination: `${RAW}/install.sh`, permanent: false },
      { source: "/install.ps1", destination: `${RAW}/install.ps1`, permanent: false },
    ];
  },
};

export default nextConfig;
