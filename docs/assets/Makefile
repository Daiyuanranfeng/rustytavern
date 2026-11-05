LATEXMK ?= latexmk
PDFTOCAIRO ?= pdftocairo
SOURCES := product-map.tex runtime-architecture.tex generation-workflow.tex database-model.tex
PDFS := $(SOURCES:.tex=.pdf)
SVGS := $(SOURCES:.tex=.svg)

.PHONY: all clean
all: $(PDFS) $(SVGS)

%.pdf: %.tex
	$(LATEXMK) -pdf -interaction=nonstopmode -halt-on-error $<

%.svg: %.pdf
	$(PDFTOCAIRO) -svg $< $@

clean:
	$(LATEXMK) -c $(SOURCES)
