FROM registry.fedoraproject.org/fedora:43

# When building for multiple-architectures in parallel using emulation
# it's really easy for one/more dnf processes to timeout or mis-count
# the minimum download rates.  Bump both to be extremely forgiving of
# an overworked host.
RUN echo -e "\n\n# Added during image build" >> /etc/dnf/dnf.conf && \
    echo -e "minrate=100\ntimeout=60\n" >> /etc/dnf/dnf.conf

ARG INSTALL_RPMS="podman podman-docker bubblewrap fuse-overlayfs slirp4netns passt openssh-clients openssl cpp git-core sqlite python3 python3-pip python3-pytest nodejs ripgrep jq gcc gcc-c++ make procps-ng gh glab ShellCheck python3-pyyaml unzip perl 7zip"
ARG BASEDPYRIGHT_NPM_VERSION="1.38.4"
ARG CODEX_NPM_REFRESH_TOKEN="static"

# Don't include container-selinux and remove
# directories used by dnf that are just taking
# up space.
# TODO: rpm --setcaps... needed due to Fedora (base) image builds
#       being (maybe still?) affected by
#       https://bugzilla.redhat.com/show_bug.cgi?id=1995337#c3
RUN dnf -y makecache && \
    dnf -y update && \
    rpm --setcaps shadow-utils 2>/dev/null && \
    dnf -y install $INSTALL_RPMS --exclude container-selinux && \
    printf '%s\n' "$CODEX_NPM_REFRESH_TOKEN" >/dev/null && \
    npm install -g @openai/codex@latest basedpyright@$BASEDPYRIGHT_NPM_VERSION && \
    dnf clean all && \
    rm -fv /etc/machine-id /var/lib/systemd/random-seed /var/lib/dnf/repos/*/countme && \
    rm -fv /usr/lib/systemd/profile.d/* && \
    rm -rf /var/cache /var/log/dnf* /var/log/hawkey.log /var/log/yum.*

RUN echo -e "root:1:65535" > /etc/subuid && \
    echo -e "root:1:65535" > /etc/subgid

RUN mkdir -p /tmp/podman-run-0 /root/.config/containers && \
    ln -sf /tmp/podman-run-0/podman/podman.sock /var/run/docker.sock

ADD /containers.conf /etc/containers/containers.conf
ADD /podman-containers.conf /root/.config/containers/containers.conf
ADD /container-entrypoint.sh /usr/local/bin/container-entrypoint.sh

RUN chmod 644 /etc/containers/containers.conf && \
    chmod 755 /usr/local/bin/container-entrypoint.sh

# Copy & modify the defaults to provide reference if runtime changes needed.
# Changes here are required for running with fuse-overlay storage inside container.
RUN sed -e 's|^#mount_program|mount_program|g' \
           -e '/additionalimage.*/a "/var/lib/shared",' \
           -e 's|^mountopt[[:space:]]*=.*$|mountopt = "nodev,fsync=0"|g' \
           /usr/share/containers/storage.conf \
           > /etc/containers/storage.conf

# Setup internal Podman to pass subscriptions down from host to internal container
RUN printf '/run/secrets/etc-pki-entitlement:/run/secrets/etc-pki-entitlement\n/run/secrets/rhsm:/run/secrets/rhsm\n' > /etc/containers/mounts.conf

# Note VOLUME options must happen after preparing the backing paths
# RUN commands can not modify existing volumes
VOLUME /var/lib/containers

RUN mkdir -p /var/lib/shared/overlay-images \
             /var/lib/shared/overlay-layers \
             /var/lib/shared/vfs-images \
             /var/lib/shared/vfs-layers && \
    touch /var/lib/shared/overlay-images/images.lock && \
    touch /var/lib/shared/overlay-layers/layers.lock && \
    touch /var/lib/shared/vfs-images/images.lock && \
    touch /var/lib/shared/vfs-layers/layers.lock

ENV _CONTAINERS_USERNS_CONFIGURED="" \
    BUILDAH_ISOLATION=chroot \
    XDG_RUNTIME_DIR=/tmp/podman-run-0 \
    DOCKER_HOST=unix:///tmp/podman-run-0/podman/podman.sock

ENTRYPOINT ["/usr/local/bin/container-entrypoint.sh"]
