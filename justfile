# Copyright 2018-2021 Cargill Incorporated
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.


crates := '\
    . \
    examples/intkey_rust \
    examples/xo_rust \
    '

features := '\
    --features=experimental \
    --features=stable \
    --features=default \
    --no-default-features \
    '

build:
    #!/usr/bin/env sh
    set -e
    for feature in $(echo {{features}})
    do
        for crate in $(echo {{crates}})
        do
            cmd="cargo build --tests --manifest-path=$crate/Cargo.toml $feature"
            echo "\033[1m$cmd\033[0m"
            $cmd
        done
    done
    echo "\n\033[92mBuild Success\033[0m\n"

build-docs:
    #!/usr/bin/env sh
    docker build . -f docs/Dockerfile -t sawtooth-sdk-rust-docs
    docker run --rm -v $(pwd):/project/sawtooth-sdk-rust sawtooth-sdk-rust-docs

ci:
    just ci-lint
    just ci-test
    just ci-build-docs
    just ci-build-artifacts

ci-build-artifacts:
    #!/usr/bin/env sh
    REPO_VERSION=$(VERSION=AUTO_STRICT ./bin/get_version) \
    docker-compose -f docker-compose-installed.yaml build
    docker-compose -f docker/compose/copy-debs.yaml up
    docker-compose -f docker/compose/copy-debs.yaml down

ci-build-docs: build-docs

ci-lint:
    #!/usr/bin/env sh
    docker-compose -f docker/compose/run-lint.yaml build
    docker-compose -f docker/compose/run-lint.yaml up \
        --abort-on-container-exit --exit-code-from lint-rust

ci-test:
    #!/usr/bin/env sh
    docker-compose -f docker/compose/sawtooth-build.yaml up
    docker-compose -f docker/compose/sawtooth-build.yaml down
    INSTALL_TYPE="" ./bin/run_tests

clean:
    #!/usr/bin/env sh
    set -e
    for crate in $(echo {{crates}})
    do
        cmd="cargo clean --manifest-path=$crate/Cargo.toml"
        echo "\033[1m$cmd\033[0m"
        $cmd
        cmd="rm -f $crate/Cargo.lock"
        echo "\033[1m$cmd\033[0m"
        $cmd
    done

copy-env:
    #!/usr/bin/env sh
    set -e
    find . -name .env | xargs -I '{}' sh -c "echo 'Copying to {}'; rsync .env {}"

lint: lint-ignore
    #!/usr/bin/env sh
    set -e
    echo "\033[1mcargo fmt -- --check\033[0m"
    cargo fmt -- --check
    for feature in $(echo {{features}})
    do
        for crate in $(echo {{crates}})
        do
            cmd="cargo clippy --manifest-path=$crate/Cargo.toml $feature -- -D warnings"
            echo "\033[1m$cmd\033[0m"
            $cmd
        done
    done
    echo "\n\033[92mLint Success\033[0m\n"

lint-ignore:
    #!/usr/bin/env sh
    set -e
    diff -u .dockerignore .gitignore
    echo "\n\033[92mLint Ignore Files Success\033[0m\n"

test:
    #!/usr/bin/env sh
    set -e
    for feature in $(echo {{features}})
    do
        for crate in $(echo {{crates}})
        do
            cmd="cargo build --tests --manifest-path=$crate/Cargo.toml $feature"
            echo "\033[1m$cmd\033[0m"
            $cmd
            cmd="cargo test --manifest-path=$crate/Cargo.toml $feature"
            echo "\033[1m$cmd\033[0m"
            $cmd
        done
    done
    echo "\n\033[92mTest Success\033[0m\n"
