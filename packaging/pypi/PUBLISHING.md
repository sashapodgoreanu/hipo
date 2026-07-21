# Publishing `duckle` to PyPI

Publishing runs through `.github/workflows/pypi.yml` using **Trusted Publishing (OIDC)**. No API token is created, stored, or pasted anywhere: GitHub mints a short-lived identity token for that one workflow and PyPI verifies it against a publisher you register once.

Everything below is a one-time setup. After it, publishing is: publish a GitHub release, and the wheels appear on PyPI.

> **The name `duckle` was unclaimed on PyPI as of 2026-07-21.** A PyPI version can never be re-uploaded, and a deleted release can never be reused, so the first upload is one-shot. Do the TestPyPI rehearsal in step 3 first.

---

## 1. PyPI: register a pending publisher (~3 minutes)

The project does not exist on PyPI yet, so you register a **pending** publisher. This is the step people get wrong: the per-project publisher form only appears for projects that already exist.

1. Sign in at **https://pypi.org** (PyPI requires 2FA on every account that publishes).
2. Go to **https://pypi.org/manage/account/publishing/**.
3. Find **"Add a new pending publisher"**, choose the **GitHub** tab, and enter exactly:

   | Field | Value |
   |---|---|
   | PyPI Project Name | `duckle` |
   | Owner | `slothflowlabs` |
   | Repository name | `duckle` |
   | Workflow name | `pypi.yml` |
   | Environment name | `pypi` |

4. Click **Add**.

Two details that cause a silent `invalid-publisher` rejection later:

- **Workflow name is the filename only**, `pypi.yml`, not `.github/workflows/pypi.yml`.
- **Environment name must match exactly**, including case. The workflow uses `pypi`.

The pending publisher converts into a normal one automatically on the first successful upload.

## 2. GitHub: create the environments

The workflow pins its publish job to a GitHub environment, which is what PyPI checks and what lets you gate a release behind approval.

1. Repo → **Settings** → **Environments** → **New environment**.
2. Create **`pypi`**.
3. Create **`testpypi`**.

Optional but recommended on `pypi`: enable **Required reviewers** and add yourself. The publish job then pauses and waits for you to approve, so a mistaken release cannot reach PyPI unattended.

Nothing else is needed on the GitHub side. Do **not** add a `PYPI_API_TOKEN` secret; the whole point of OIDC is that there is no token to leak.

## 3. Rehearse on TestPyPI first

TestPyPI is a throwaway index that mirrors the real one. Rehearsing there means the first time the pipeline runs is not also the first time the name is claimed.

1. Create a separate account at **https://test.pypi.org** (TestPyPI accounts are not shared with PyPI).
2. Register a pending publisher there with the same values as step 1, except **Environment name: `testpypi`**.
3. In the repo: **Actions** → **PyPI** → **Run workflow**, set **Index** to `testpypi`, and run it.
4. When it goes green, check the page renders as expected: **https://test.pypi.org/project/duckle/**, then try it in a scratch virtualenv:

   ```sh
   pip install --index-url https://test.pypi.org/simple/ \
               --extra-index-url https://pypi.org/simple/ duckle
   duckle --help
   ```

   The extra index is needed because the `duckdb-cli` dependency lives on real PyPI, not TestPyPI.

## 4. Publish for real

Two ways, both authenticated by OIDC:

**With a release (the normal path).** Publish the GitHub release for the tag. `pypi.yml` fires on `release: published`, so it runs after the desktop release workflow has finished and you have looked at the draft.

**Manually.** **Actions** → **PyPI** → **Run workflow**, set **Index** to `pypi`.

Before either, make sure `version` in `packaging/pypi/pyproject.toml` matches the release tag without the `v`. The workflow's `check-version` job fails the run if they disagree, precisely because publishing the wrong version under a name that can never be reused is unrecoverable.

---

## What the workflow does

| Job | What it does |
|---|---|
| `check-version` | Fails if `pyproject.toml` and the release tag disagree |
| `build-wheels` | 6 platform wheels, then `twine check` on each |
| `publish` | Refuses unless all 6 arrived, then uploads via OIDC |

Linux wheels are built against **musl and linked fully static**, so the binary carries no libc dependency and honestly satisfies both the manylinux and musllinux tags. Building natively on `ubuntu-24.04` would link glibc 2.39 and could not legitimately claim `manylinux_2_17`, which is what makes a wheel installable on older distros.

macOS wheels set `MACOSX_DEPLOYMENT_TARGET=11.0` to match the `macosx_11_0` tag, so the wheel does not advertise support for a system the binary will not launch on.

## Troubleshooting

**`invalid-publisher` / `not a valid token`** - the OIDC claim did not match. Check the workflow filename, the owner and repo spelling, and above all the environment name, which must be identical on both sides.

**`403 Forbidden` on upload** - usually the pending publisher was registered for a different project name, or the version already exists. PyPI never allows re-uploading a version, even after deleting it.

**Fewer than 6 wheels** - the publish job stops on purpose rather than shipping a partial platform set, which would leave `pip install duckle` failing for whoever is on the missing platform. Re-run the failed matrix leg.

**Bumping the version** - edit `version` in `packaging/pypi/pyproject.toml` and `__version__` in `packaging/pypi/duckle/__init__.py`. They should agree with the release tag.
