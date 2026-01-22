IMAGE ?= canmi/vane
TAG ?= $(shell git rev-parse --short HEAD)

.PHONY: build push pushrm clean

build:
	docker build -t $(IMAGE):$(TAG) -t $(IMAGE):latest -f Dockerfile .

push:
	docker buildx build \
		--platform linux/amd64,linux/arm64 \
		-t $(IMAGE):$(TAG) \
		-t $(IMAGE):latest \
		-f Dockerfile . --push

pushrm:
	docker pushrm $(IMAGE)

clean:
	docker buildx prune -f
